//! Reference MODBUS RTU slave — conforming by construction — plus a catalog
//! of deliberate contract violations ([`SlaveBugs`]) for negative validation
//! of the oracle and the conformance suite.

use stablehal_contract::Nanos;
use stablehal_uart::UartBus;

use crate::crc;
use crate::pdu::{self, Adu, ExceptionCode, ParsedRequest};
use crate::timing::RtuTiming;

/// Holding-register space of the reference slave.
pub const BANK_SIZE: usize = 128;

#[derive(Clone)]
pub struct RegisterBank {
    regs: [u16; BANK_SIZE],
}

impl RegisterBank {
    pub fn new() -> Self {
        RegisterBank {
            regs: [0; BANK_SIZE],
        }
    }
    /// Initialize every register from its address (conformance precondition).
    pub fn with_pattern(f: impl Fn(u16) -> u16) -> Self {
        let mut b = RegisterBank::new();
        for i in 0..BANK_SIZE {
            b.regs[i] = f(i as u16);
        }
        b
    }
    pub fn get(&self, addr: u16) -> u16 {
        self.regs[addr as usize]
    }
    pub fn set(&mut self, addr: u16, value: u16) {
        self.regs[addr as usize] = value;
    }
}

impl Default for RegisterBank {
    fn default() -> Self {
        Self::new()
    }
}

/// Deliberate contract violations. All `false`/`None` = reference behavior.
#[derive(Copy, Clone, Debug, Default)]
pub struct SlaveBugs {
    /// Respond before t3.5 has elapsed after the request's last character
    /// (violates frame separation — caught as RTU-T35-FRAME).
    pub respond_inside_t35: bool,
    /// Emit a response whose CRC is wrong (RTU-CRC).
    pub corrupt_crc: bool,
    /// Insert this much extra silence between response characters
    /// (> t1.5 ⇒ RTU-T15-INTRA).
    pub intra_frame_gap: Option<Nanos>,
    /// Delay the response this far beyond the response timeout
    /// (RTU-RESP-TIME).
    pub late_by: Option<Nanos>,
    /// Answer with a function code unrelated to the request (RTU-RESP-FC).
    pub wrong_fc: bool,
    /// Answer with a different unit address (RTU-RESP-UNIT).
    pub wrong_unit: bool,
}

#[derive(Copy, Clone, Debug)]
pub struct SlaveConfig {
    pub unit: u8,
    pub timing: RtuTiming,
    /// Extra think time beyond the mandatory t3.5, before the response
    /// starts. Must keep the start inside the contract envelope.
    pub response_delay: Nanos,
    pub bugs: SlaveBugs,
}

/// Result of one poll: earliest useful next poll time (`None` = wake on RX).
#[derive(Copy, Clone, Debug)]
pub struct SlaveProgress {
    pub wake_at: Option<Nanos>,
}

#[derive(Clone, Debug)]
enum SState {
    Listening,
    Responding {
        idx: usize,
        next_at: Nanos,
        gap_inserted: bool,
        resp: Adu,
    },
}

pub struct RtuSlave {
    cfg: SlaveConfig,
    pub bank: RegisterBank,
    state: SState,
    req: Adu,
    req_poisoned: bool,
    last_rx: Option<Nanos>,
}

/// Recompute and rewrite the CRC after a deliberate field mutation, so a
/// buggy slave violates exactly one clause at a time.
fn refresh_crc(adu: &mut Adu) {
    let len = adu.len();
    let crc = crc::crc16(&adu.bytes()[..len - 2]);
    *adu.byte_mut(len - 2) = (crc & 0xFF) as u8;
    *adu.byte_mut(len - 1) = (crc >> 8) as u8;
}

/// Expected total ADU length for a (possibly still incomplete) request, if
/// derivable. Used only by the `respond_inside_t35` bug, which "cleverly"
/// detects frame completion by length instead of waiting out t3.5.
fn expected_request_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 2 {
        return None;
    }
    match bytes[1] {
        pdu::FC_READ_HOLDING | pdu::FC_WRITE_SINGLE => Some(8),
        pdu::FC_WRITE_MULTIPLE => {
            if bytes.len() >= 7 {
                Some(9 + bytes[6] as usize)
            } else {
                None
            }
        }
        _ => None,
    }
}

impl RtuSlave {
    pub fn new(cfg: SlaveConfig, bank: RegisterBank) -> Self {
        RtuSlave {
            cfg,
            bank,
            state: SState::Listening,
            req: Adu::new(),
            req_poisoned: false,
            last_rx: None,
        }
    }

    /// Service a validated request against the register bank. AP §6.3, §6.6,
    /// §6.12, §7 — including the mandated exception-check order
    /// (function → value → address).
    fn service(&mut self, req: &ParsedRequest<'_>, out: &mut Adu) {
        let unit = self.cfg.unit;
        match *req {
            ParsedRequest::ReadHolding { addr, count } => {
                if !(1..=125).contains(&count) {
                    pdu::build_exception(
                        unit,
                        pdu::FC_READ_HOLDING,
                        ExceptionCode::IllegalDataValue,
                        out,
                    );
                } else if addr as usize + count as usize > BANK_SIZE {
                    pdu::build_exception(
                        unit,
                        pdu::FC_READ_HOLDING,
                        ExceptionCode::IllegalDataAddress,
                        out,
                    );
                } else {
                    let mut values = [0u16; 125];
                    for i in 0..count {
                        values[i as usize] = self.bank.get(addr + i);
                    }
                    pdu::build_read_response(unit, &values[..count as usize], out);
                }
            }
            ParsedRequest::WriteSingle { addr, value } => {
                if addr as usize >= BANK_SIZE {
                    pdu::build_exception(
                        unit,
                        pdu::FC_WRITE_SINGLE,
                        ExceptionCode::IllegalDataAddress,
                        out,
                    );
                } else {
                    self.bank.set(addr, value);
                    pdu::build_write_single_response(unit, addr, value, out);
                }
            }
            ParsedRequest::WriteMultiple { addr, count, data } => {
                if !(1..=123).contains(&count) {
                    pdu::build_exception(
                        unit,
                        pdu::FC_WRITE_MULTIPLE,
                        ExceptionCode::IllegalDataValue,
                        out,
                    );
                } else if addr as usize + count as usize > BANK_SIZE {
                    pdu::build_exception(
                        unit,
                        pdu::FC_WRITE_MULTIPLE,
                        ExceptionCode::IllegalDataAddress,
                        out,
                    );
                } else {
                    for i in 0..count as usize {
                        let v = (u16::from(data[i * 2]) << 8) | u16::from(data[i * 2 + 1]);
                        self.bank.set(addr + i as u16, v);
                    }
                    pdu::build_write_multiple_response(unit, addr, count, out);
                }
            }
            ParsedRequest::UnknownFunction { fc } => {
                pdu::build_exception(unit, fc, ExceptionCode::IllegalFunction, out);
            }
        }
    }

    /// The request frame is complete (last character ended at `frame_end`).
    /// Returns the wake time if a response was scheduled.
    fn process_frame(&mut self, frame_end: Nanos) -> Option<Nanos> {
        let poisoned = self.req_poisoned;
        let frame = self.req.clone();
        self.req.clear();
        self.req_poisoned = false;
        self.last_rx = None;

        if poisoned {
            return None; // corrupted by the line; discard silently
        }
        let (unit, parsed) = match pdu::parse_request(frame.bytes()) {
            Ok(x) => x,
            Err(_) => return None, // CRC/format garbage; discard silently
        };
        if unit != self.cfg.unit {
            return None; // not addressed to us
        }

        let mut resp = Adu::new();
        let parsed_owned = parsed; // borrows `frame`, used immediately
        self.service(&parsed_owned, &mut resp);

        // Apply the bug catalog (each recomputes CRC where needed so exactly
        // one clause is broken).
        let bugs = self.cfg.bugs;
        if bugs.wrong_unit {
            *resp.byte_mut(0) = self.cfg.unit.wrapping_add(1);
            refresh_crc(&mut resp);
        }
        if bugs.wrong_fc {
            *resp.byte_mut(1) = resp.bytes()[1].wrapping_add(0x21); // neither fc nor fc|0x80
            refresh_crc(&mut resp);
        }
        if bugs.corrupt_crc {
            let last = resp.len() - 1;
            *resp.byte_mut(last) ^= 0xFF;
        }

        let mut start_at = if bugs.respond_inside_t35 {
            frame_end + self.cfg.timing.t35.mul_ratio(1, 3)
        } else {
            frame_end + self.cfg.timing.t35 + self.cfg.response_delay
        };
        if let Some(extra) = bugs.late_by {
            start_at = frame_end + self.cfg.timing.response_timeout + extra;
        }

        self.state = SState::Responding {
            idx: 0,
            next_at: start_at,
            gap_inserted: false,
            resp,
        };
        Some(start_at)
    }

    pub fn poll<B: UartBus>(&mut self, now: Nanos, bus: &mut B) -> SlaveProgress {
        while let Some(item) = bus.try_read() {
            if let SState::Listening = self.state {
                match item {
                    Ok(b) => self.req.push(b),
                    Err(_) => self.req_poisoned = true,
                }
                self.last_rx = Some(now);
            }
            // While responding, incoming bytes are a bus anomaly; ignore.
        }

        match &mut self.state {
            SState::Listening => {
                let Some(lr) = self.last_rx else {
                    return SlaveProgress { wake_at: None };
                };
                if self.req.is_empty() {
                    return SlaveProgress { wake_at: None };
                }
                // The buggy "fast" slave detects frame completion by length
                // instead of waiting out the t3.5 silence.
                let early_complete = self.cfg.bugs.respond_inside_t35
                    && expected_request_len(self.req.bytes()) == Some(self.req.len());
                let frame_complete_at = lr + self.cfg.timing.t35;
                if early_complete || now >= frame_complete_at {
                    let wake = self.process_frame(lr);
                    SlaveProgress { wake_at: wake }
                } else {
                    SlaveProgress {
                        wake_at: Some(frame_complete_at),
                    }
                }
            }
            SState::Responding {
                idx,
                next_at,
                gap_inserted,
                resp,
            } => {
                if now >= *next_at && *idx < resp.len() {
                    let byte = resp.bytes()[*idx];
                    if bus.try_write(byte).is_ok() {
                        *idx += 1;
                        *next_at = now + self.cfg.timing.char_time;
                        if let Some(gap) = self.cfg.bugs.intra_frame_gap {
                            if *idx == 3 && !*gap_inserted {
                                *next_at = *next_at + gap;
                                *gap_inserted = true;
                            }
                        }
                    } else {
                        *next_at = now + self.cfg.timing.char_time;
                    }
                }
                if *idx == resp.len() {
                    self.state = SState::Listening;
                    self.last_rx = None;
                    SlaveProgress { wake_at: None }
                } else {
                    SlaveProgress {
                        wake_at: Some(*next_at),
                    }
                }
            }
        }
    }
}
