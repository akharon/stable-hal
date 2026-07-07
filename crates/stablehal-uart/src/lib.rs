//! UART HAL contract.
//!
//! Two faces, one agreement:
//! * [`UartBus`] — the SW-facing HAL trait drivers are written against
//!   (embedded-hal style, poll-driven, `no_std`).
//! * [`LineContract`] — the wire-facing obligations of whoever transmits on
//!   one simplex line: exact character timing, no overlapping characters,
//!   start/end pairing. The simulator and (later) a hardware bus observer are
//!   judged by the *same* contract.
//!
//! A full-duplex link is two independent simplex lines, each with its own
//! `LineContract` instance.

#![no_std]
#![deny(unsafe_code)]

use stablehal_contract::{Breach, Clause, Contract, Nanos, TimeBound};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UartConfig {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
}

impl UartConfig {
    /// MODBUS RTU default framing: 8E1 (11 bits per character on the wire).
    pub const fn rtu_default(baud: u32) -> Self {
        UartConfig {
            baud,
            data_bits: 8,
            parity: Parity::Even,
            stop_bits: 1,
        }
    }

    pub const fn bits_per_char(&self) -> u32 {
        let parity_bits = match self.parity {
            Parity::None => 0,
            _ => 1,
        };
        1 + self.data_bits as u32 + parity_bits + self.stop_bits as u32
    }

    /// Wire occupancy of one character. Integer nanoseconds, truncated —
    /// every component (driver, simulator, oracle, conformance generator)
    /// must use this same function so bounds agree to the nanosecond.
    pub const fn char_time(&self) -> Nanos {
        Nanos::from_nanos(self.bits_per_char() as u64 * 1_000_000_000 / self.baud as u64)
    }
}

// ---------------------------------------------------------------------------
// SW-facing HAL trait
// ---------------------------------------------------------------------------

/// Receive-side faults a UART can report to software. This is the complete
/// fault vocabulary of the contract: a conforming implementation reports
/// nothing outside this set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UartError {
    Framing,
    Parity,
    Noise,
    Overrun,
}

/// Returned by `try_write` while a character is still being shifted out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TxBusy;

/// The SW-facing UART HAL. Drivers (e.g. the MODBUS RTU master) are written
/// against this trait only; the simulator and real hardware both implement it.
pub trait UartBus {
    /// Begin transmitting one byte. Fails while the previous byte is still on
    /// the wire (single-depth TX, no FIFO — keeps timing observable).
    fn try_write(&mut self, byte: u8) -> Result<(), TxBusy>;
    /// Pop the next received byte or fault, if any arrived.
    fn try_read(&mut self) -> Option<Result<u8, UartError>>;
}

// ---------------------------------------------------------------------------
// Wire-facing contract (one simplex line)
// ---------------------------------------------------------------------------

/// Observable events on one simplex line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineEvent {
    /// Transmitter began driving a character.
    TxStart { byte: u8 },
    /// Transmitter released the line; the character is complete.
    TxEnd { byte: u8 },
    /// Receiver-side delivery after propagation (environment event; carries
    /// no transmitter obligation, recorded for trace completeness).
    Delivered { byte: u8 },
    /// Receiver-side delivery of an environment-corrupted character.
    DeliveredCorrupt { error: UartError },
}

pub static CL_UART_NO_OVERLAP: Clause = Clause {
    id: "UART-NO-OVERLAP",
    citation: "UART character framing (async serial, 16550 semantics)",
    text: "A transmitter must not start a character while the previous character is still occupying the line.",
};

pub static CL_UART_CHAR_TIME: Clause = Clause {
    id: "UART-CHAR-TIME",
    citation: "UART character framing (async serial, 16550 semantics)",
    text: "A character occupies the line for exactly bits_per_char/baud (within the configured tolerance).",
};

pub static CL_UART_TX_PAIRING: Clause = Clause {
    id: "UART-TX-PAIRING",
    citation: "UART character framing (async serial, 16550 semantics)",
    text: "Every TxEnd must terminate the most recent TxStart and carry the same byte.",
};

static UART_CLAUSES: [&Clause; 3] = [&CL_UART_NO_OVERLAP, &CL_UART_CHAR_TIME, &CL_UART_TX_PAIRING];

/// Contract over one simplex line's transmitter.
#[derive(Copy, Clone, Debug)]
pub struct LineContract {
    pub config: UartConfig,
    /// Allowed deviation of measured char time from nominal (clock tolerance,
    /// typically ≤ 2% of char time). A zero tolerance demands exactness.
    pub char_time_tolerance: Nanos,
}

impl LineContract {
    pub fn new(config: UartConfig) -> Self {
        // Default tolerance: 2% of nominal char time.
        let tol = Nanos::from_nanos(config.char_time().as_nanos() / 50);
        LineContract {
            config,
            char_time_tolerance: tol,
        }
    }

    fn char_envelope(&self) -> TimeBound {
        let nominal = self.config.char_time();
        TimeBound::new(
            nominal.saturating_sub(self.char_time_tolerance),
            nominal.saturating_add(self.char_time_tolerance),
        )
    }
}

#[derive(Clone, Debug)]
pub struct LineState {
    /// Character currently on the line: (byte, started_at).
    in_flight: Option<(u8, Nanos)>,
    /// Time at which the line last became free.
    line_free_at: Nanos,
}

impl Contract for LineContract {
    type State = LineState;
    type Event = LineEvent;

    fn initial(&self) -> LineState {
        LineState {
            in_flight: None,
            line_free_at: Nanos::ZERO,
        }
    }

    fn step(&self, state: &mut LineState, at: Nanos, event: &LineEvent) -> Result<(), Breach> {
        match *event {
            LineEvent::TxStart { byte } => {
                if state.in_flight.is_some() || at < state.line_free_at {
                    return Err(Breach::timing(
                        &CL_UART_NO_OVERLAP,
                        at.saturating_sub(state.line_free_at),
                        TimeBound::at_least(Nanos::ZERO),
                    ));
                }
                state.in_flight = Some((byte, at));
                Ok(())
            }
            LineEvent::TxEnd { byte } => match state.in_flight.take() {
                None => Err(Breach::of(&CL_UART_TX_PAIRING)),
                Some((b, _)) if b != byte => Err(Breach::of(&CL_UART_TX_PAIRING)),
                Some((_, started)) => {
                    let dur = at.saturating_sub(started);
                    let envelope = self.char_envelope();
                    if !envelope.contains(dur) {
                        return Err(Breach::timing(&CL_UART_CHAR_TIME, dur, envelope));
                    }
                    state.line_free_at = at;
                    Ok(())
                }
            },
            // Environment-side events: no transmitter obligation.
            LineEvent::Delivered { .. } | LineEvent::DeliveredCorrupt { .. } => Ok(()),
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &UART_CLAUSES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::Oracle;

    const CFG: UartConfig = UartConfig::rtu_default(9600);

    #[test]
    fn char_time_9600_8e1_is_11_bits() {
        assert_eq!(CFG.bits_per_char(), 11);
        assert_eq!(CFG.char_time(), Nanos::from_nanos(1_145_833));
    }

    #[test]
    fn conforming_transmission_passes() {
        let ct = CFG.char_time();
        let mut o = Oracle::new(LineContract::new(CFG));
        let t0 = Nanos::from_millis(1);
        o.observe(t0, &LineEvent::TxStart { byte: 0x01 }).unwrap();
        o.observe(t0 + ct, &LineEvent::TxEnd { byte: 0x01 }).unwrap();
        o.observe(t0 + ct, &LineEvent::TxStart { byte: 0x02 }).unwrap();
        o.observe(t0 + ct + ct, &LineEvent::TxEnd { byte: 0x02 })
            .unwrap();
    }

    #[test]
    fn overlapping_characters_breach_no_overlap() {
        let mut o = Oracle::new(LineContract::new(CFG));
        let t0 = Nanos::from_millis(1);
        o.observe(t0, &LineEvent::TxStart { byte: 0x01 }).unwrap();
        let v = o
            .observe(t0 + Nanos::from_micros(10), &LineEvent::TxStart { byte: 0x02 })
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "UART-NO-OVERLAP");
    }

    #[test]
    fn wrong_char_duration_breaches_char_time() {
        let mut o = Oracle::new(LineContract::new(CFG));
        let t0 = Nanos::from_millis(1);
        o.observe(t0, &LineEvent::TxStart { byte: 0x01 }).unwrap();
        let v = o
            .observe(t0 + Nanos::from_micros(100), &LineEvent::TxEnd { byte: 0x01 })
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "UART-CHAR-TIME");
    }
}
