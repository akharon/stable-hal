//! Contract-conforming MODBUS RTU master driver. Written against
//! [`UartBus`] + virtual time only — the identical code is exercised under
//! DST on the host and can run on an MCU target.

use stablehal_contract::Nanos;
use stablehal_uart::UartBus;

use crate::pdu::{self, Adu, ExceptionCode, Request};
use crate::timing::RtuTiming;

#[derive(Copy, Clone, Debug)]
pub struct MasterConfig {
    pub timing: RtuTiming,
    /// Retransmissions after the first attempt (SL §2.4.1 allows retries
    /// after the response timeout).
    pub retries: u8,
    /// PLANTED-BUG KNOB (negative validation only): skip the t3.5 silence
    /// wait before transmitting. A correct configuration is `false`.
    pub skip_pre_tx_silence: bool,
}

impl MasterConfig {
    pub fn new(timing: RtuTiming) -> Self {
        MasterConfig {
            timing,
            retries: 2,
            skip_pre_tx_silence: false,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MasterError {
    /// No (valid) response after all attempts.
    Timeout,
    /// Response present but unusable after all attempts (CRC/format).
    BadFrame,
    /// The slave answered with an exception.
    Exception(ExceptionCode),
}

/// Result of one poll.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
// ADR-9: Adu is a fixed 256-byte buffer; the size difference across
// MasterProgress variants is accepted knowingly.
pub enum MasterProgress {
    /// Transaction in flight. `wake_at`: earliest time another poll is
    /// useful (`None` = only incoming bytes can advance the driver).
    Busy { wake_at: Option<Nanos> },
    /// Transaction finished; on success the validated response ADU.
    Done(Result<Adu, MasterError>),
}

#[derive(Clone, Debug)]
enum MState {
    Idle,
    WaitSilence,
    Sending {
        idx: usize,
        next_at: Nanos,
    },
    AwaitResponse {
        deadline: Nanos,
        last_rx: Option<Nanos>,
        poisoned: bool,
    },
}

pub struct RtuMaster {
    cfg: MasterConfig,
    state: MState,
    request: Adu,
    expect_unit: u8,
    expect_fc: u8,
    attempts_left: u8,
    response: Adu,
    /// Last time we saw or produced wire activity; silence is measured
    /// against this.
    last_activity: Nanos,
}

impl RtuMaster {
    pub fn new(cfg: MasterConfig) -> Self {
        RtuMaster {
            cfg,
            state: MState::Idle,
            request: Adu::new(),
            expect_unit: 0,
            expect_fc: 0,
            attempts_left: 0,
            response: Adu::new(),
            last_activity: Nanos::ZERO,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.state, MState::Idle)
    }

    /// Begin a transaction. Returns false while a previous one is in flight.
    pub fn start(&mut self, unit: u8, req: &Request<'_>) -> bool {
        if !self.is_idle() {
            return false;
        }
        pdu::build_request(unit, req, &mut self.request);
        self.expect_unit = unit;
        self.expect_fc = req.function_code();
        self.attempts_left = self.cfg.retries + 1;
        self.response.clear();
        self.state = MState::WaitSilence;
        true
    }

    /// One retry attempt failed; either rearm or finish with `err`.
    fn attempt_failed(&mut self, err: MasterError) -> MasterProgress {
        self.attempts_left -= 1;
        if self.attempts_left == 0 {
            self.state = MState::Idle;
            MasterProgress::Done(Err(err))
        } else {
            self.response.clear();
            self.state = MState::WaitSilence;
            MasterProgress::Busy {
                wake_at: Some(self.last_activity + self.cfg.timing.t35),
            }
        }
    }

    pub fn poll<B: UartBus>(&mut self, now: Nanos, bus: &mut B) -> MasterProgress {
        // Drain the receiver first. Outside AwaitResponse the bytes are
        // stray (late responses, noise): discard, but note the activity so
        // the pre-TX silence window restarts.
        while let Some(item) = bus.try_read() {
            self.last_activity = now;
            if let MState::AwaitResponse {
                last_rx, poisoned, ..
            } = &mut self.state
            {
                match item {
                    Ok(b) => self.response.push(b),
                    Err(_) => *poisoned = true,
                }
                *last_rx = Some(now);
            }
        }

        loop {
            match self.state {
                MState::Idle => return MasterProgress::Busy { wake_at: None },

                MState::WaitSilence => {
                    let ready = if self.cfg.skip_pre_tx_silence {
                        now
                    } else {
                        self.last_activity + self.cfg.timing.t35
                    };
                    if now >= ready {
                        self.state = MState::Sending {
                            idx: 0,
                            next_at: now,
                        };
                        continue;
                    }
                    return MasterProgress::Busy {
                        wake_at: Some(ready),
                    };
                }

                MState::Sending { idx, next_at } => {
                    if idx == self.request.len() {
                        // Last character completes at `next_at`: the request
                        // is on the wire in full at that instant.
                        let request_end = next_at;
                        self.last_activity = request_end;
                        let deadline = request_end + self.cfg.timing.response_timeout;
                        self.state = MState::AwaitResponse {
                            deadline,
                            last_rx: None,
                            poisoned: false,
                        };
                        return MasterProgress::Busy {
                            wake_at: Some(deadline),
                        };
                    }
                    if now < next_at {
                        return MasterProgress::Busy {
                            wake_at: Some(next_at),
                        };
                    }
                    let byte = self.request.bytes()[idx];
                    match bus.try_write(byte) {
                        Ok(()) => {
                            self.state = MState::Sending {
                                idx: idx + 1,
                                next_at: now + self.cfg.timing.char_time,
                            };
                            continue;
                        }
                        Err(_) => {
                            // TX unexpectedly busy; retry shortly (bounded,
                            // deterministic backoff).
                            return MasterProgress::Busy {
                                wake_at: Some(now + self.cfg.timing.char_time),
                            };
                        }
                    }
                }

                MState::AwaitResponse {
                    deadline,
                    last_rx,
                    poisoned,
                } => {
                    if let Some(lr) = last_rx {
                        let frame_end_at = lr + self.cfg.timing.t35;
                        if now < frame_end_at {
                            return MasterProgress::Busy {
                                wake_at: Some(frame_end_at),
                            };
                        }
                        // t3.5 of silence: the response frame is complete.
                        if poisoned {
                            return self.attempt_failed(MasterError::BadFrame);
                        }
                        match pdu::parse_response(
                            self.expect_unit,
                            self.expect_fc,
                            self.response.bytes(),
                        ) {
                            Ok(pdu::ParsedResponse::Exception { code }) => {
                                self.state = MState::Idle;
                                return MasterProgress::Done(Err(MasterError::Exception(code)));
                            }
                            Ok(_) => {
                                self.state = MState::Idle;
                                let resp = self.response.clone();
                                return MasterProgress::Done(Ok(resp));
                            }
                            Err(_) => {
                                return self.attempt_failed(MasterError::BadFrame);
                            }
                        }
                    }
                    if now >= deadline {
                        return self.attempt_failed(MasterError::Timeout);
                    }
                    return MasterProgress::Busy {
                        wake_at: Some(deadline),
                    };
                }
            }
        }
    }
}
