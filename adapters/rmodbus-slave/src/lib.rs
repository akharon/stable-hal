//! Adapter: rmodbus server behind the StableHAL UART boundary.
//!
//! WP1 / whitepaper E5: put a popular ecosystem MODBUS implementation
//! behind the StableHAL boundary *unmodified* and run STANDARD_SUITE
//! against it.
//!
//! Attribution split (what each side of the loop actually certifies):
//! * Wire-timing clauses (RTU-T35-FRAME, RTU-T15-INTRA, RTU-RESP-TIME)
//!   certify **the adapter's** RX-framing and TX-pacing code (mirrors
//!   RtuSlave).
//! * Frame/semantic clauses (CRC, RESP-UNIT, RESP-FC, EXC-CODE, PDU-FORM)
//!   and all Expected outcomes certify **rmodbus itself**.

use stablehal_contract::Nanos;
use stablehal_modbus::pdu::Adu;
use stablehal_modbus::timing::RtuTiming;
use stablehal_sim::{Actor, NextPoll, UartPort};
use stablehal_uart::UartBus;

use rmodbus::server::context::ModbusContext;
use rmodbus::server::storage::ModbusStorageFull;
use rmodbus::server::ModbusFrame;
use rmodbus::ModbusProto;

/// Adapter: rmodbus server behind the StableHAL UART boundary.
pub struct RmodbusSlaveActor {
    unit: u8,
    timing: RtuTiming,
    char_time: Nanos,
    /// Extra think time beyond t3.5 before the response starts.
    response_delay: Nanos,
    /// Register storage (rmodbus context).
    ctx: ModbusStorageFull,
    // RX accumulation
    req: Adu,
    req_poisoned: bool,
    last_rx: Option<Nanos>,
    // TX state
    state: RState,
}

enum RState {
    Listening,
    Responding {
        idx: usize,
        next_at: Nanos,
        resp: Vec<u8>,
    },
}

impl RmodbusSlaveActor {
    /// `pattern` initializes the holding-register context —
    /// the suite precondition.
    pub fn new(
        unit: u8,
        timing: RtuTiming,
        response_delay: Nanos,
        pattern: fn(u16) -> u16,
    ) -> Self {
        let mut ctx = ModbusStorageFull::new();
        // Initialize holding registers per the conformance precondition.
        for addr in 0..=u16::MAX {
            if ctx.set_holding(addr, pattern(addr)).is_err() {
                break;
            }
        }
        RmodbusSlaveActor {
            unit,
            timing,
            char_time: timing.char_time,
            response_delay,
            ctx,
            req: Adu::new(),
            req_poisoned: false,
            last_rx: None,
            state: RState::Listening,
        }
    }

    /// Process a completed frame through rmodbus, collecting the response.
    fn process_frame(&mut self, frame_end: Nanos) -> Option<Nanos> {
        if self.req_poisoned {
            self.req.clear();
            self.req_poisoned = false;
            self.last_rx = None;
            return None;
        }
        let bytes = self.req.bytes().to_vec();
        self.req.clear();
        self.last_rx = None;

        // Hand raw ADU bytes to rmodbus. Do NOT pre-filter unit or CRC —
        // whether rmodbus discards them is exactly what the suite measures.
        let mut response = Vec::new();
        let mut frame = ModbusFrame::new(self.unit, &bytes, ModbusProto::Rtu, &mut response);
        if frame.parse().is_err() {
            // rmodbus rejected the frame (bad CRC, broken, etc.) — silent discard.
            return None;
        }
        if frame.processing_required {
            let result = if frame.readonly {
                frame.process_read(&self.ctx)
            } else {
                frame.process_write(&mut self.ctx)
            };
            if result.is_err() {
                // Processing error (e.g. out of bounds when calling process_read);
                // rmodbus should have set frame.error — finalize_response will
                // produce the exception frame.
            }
        }
        if !frame.response_required {
            return None;
        }
        if frame.finalize_response().is_err() {
            return None;
        }

        let resp_bytes: Vec<u8> = response.clone();

        let start_at = frame_end + self.timing.t35 + self.response_delay;
        self.state = RState::Responding {
            idx: 0,
            next_at: start_at,
            resp: resp_bytes,
        };
        Some(start_at)
    }
}

impl Actor for RmodbusSlaveActor {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        // Drain incoming bytes.
        while let Some(item) = port.try_read() {
            if let RState::Listening = self.state {
                match item {
                    Ok(b) => self.req.push(b),
                    Err(_) => self.req_poisoned = true,
                }
                self.last_rx = Some(now);
            }
            // While responding, stray bytes are a bus anomaly; ignore.
        }

        match &mut self.state {
            RState::Listening => {
                let Some(lr) = self.last_rx else {
                    return NextPoll::OnRx;
                };
                if self.req.is_empty() {
                    return NextPoll::OnRx;
                }
                let frame_complete_at = lr + self.timing.t35;
                if now >= frame_complete_at {
                    if let Some(wake) = self.process_frame(lr) {
                        NextPoll::At(wake)
                    } else {
                        NextPoll::OnRx
                    }
                } else {
                    NextPoll::At(frame_complete_at)
                }
            }
            RState::Responding { idx, next_at, resp } => {
                if *idx >= resp.len() {
                    // Response fully sent.
                    self.state = RState::Listening;
                    return NextPoll::OnRx;
                }
                if now < *next_at {
                    return NextPoll::At(*next_at);
                }
                if port.try_write(resp[*idx]).is_ok() {
                    *idx += 1;
                    *next_at = now + self.char_time;
                    // Stay in Responding; will be polled again for next char.
                    return NextPoll::At(*next_at);
                }
                // TX busy; retry shortly.
                NextPoll::At(now + self.char_time)
            }
        }
    }
}
