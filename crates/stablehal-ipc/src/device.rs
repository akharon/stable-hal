//! Reference IPC responder: an echo/transform service keyed by correlation
//! id, plus the planted-bug catalog used to prove the oracle has teeth.
//! Mirrors `stablehal_i2c::RegisterPointerDevice` / `stablehal_spi::
//! SpiRegisterDevice` in role.
//!
//! Unlike SPI — where a slave has no hook to misbehave on wire-level clauses
//! (see that crate's doc comment) — an IPC responder controls almost every
//! obligation it is judged on: when it answers, what correlation id it
//! answers with, whether it answers twice, and whether it answers at all.
//! The bug catalog is correspondingly rich, and every entry maps to exactly
//! one clause (G3).

use crate::{Datagram, IpcResponder};

/// All-`false` is a conforming responder. Each bug breaks exactly one
/// clause, so the closed-loop test can assert the oracle names that clause
/// and no other.
#[derive(Copy, Clone, Debug, Default)]
pub struct IpcResponderBugs {
    /// Answer with a correlation id one higher than the request's —
    /// breaches `IPC-CORRELATION`.
    pub wrong_correlation: bool,
    /// Never answer at all — surfaces as `IPC-RESP-TIME` at end of
    /// observation (the "silent hang" failure mode).
    pub never_answer: bool,
    /// Answer the same request twice — breaches `IPC-NO-DUPLICATE`.
    pub answer_twice: bool,
    /// Emit a response longer than the channel's maximum — breaches
    /// `IPC-MSG-MAXLEN`.
    pub oversize_response: bool,
    /// Take far longer than the response envelope allows (the simulated
    /// scheduler stall) — breaches `IPC-RESP-TIME`.
    pub slow_response: bool,
    /// Return a wrong payload. A **data-correctness** bug, deliberately
    /// *not* a channel-contract breach — the same honest distinction as
    /// `I2cSlaveBugs::wrong_read_data` / `SpiSlaveBugs::wrong_read_data`.
    pub wrong_payload: bool,
}

/// A minimal request/response service: echoes the request payload with
/// every byte incremented, under the request's correlation id.
pub struct IpcResponderDevice {
    bugs: IpcResponderBugs,
    /// Number of requests served, for assertions.
    served: u32,
    alive: bool,
}

impl IpcResponderDevice {
    pub fn new() -> Self {
        IpcResponderDevice {
            bugs: IpcResponderBugs::default(),
            served: 0,
            alive: true,
        }
    }

    pub fn with_bugs(bugs: IpcResponderBugs) -> Self {
        IpcResponderDevice {
            bugs,
            ..IpcResponderDevice::new()
        }
    }

    pub fn bugs(&self) -> IpcResponderBugs {
        self.bugs
    }

    pub fn served(&self) -> u32 {
        self.served
    }

    pub fn alive(&self) -> bool {
        self.alive
    }

    /// The conforming answer to `req`: same correlation id, each payload
    /// byte incremented. Exposed so tests can state the expected value
    /// without duplicating the transform.
    pub fn expected_response(req: &Datagram) -> Datagram {
        let mut out = *req;
        for b in out.buf[..out.len as usize].iter_mut() {
            *b = b.wrapping_add(1);
        }
        out
    }
}

impl Default for IpcResponderDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl IpcResponder for IpcResponderDevice {
    fn on_request(&mut self, req: &Datagram) -> Option<Datagram> {
        if self.bugs.never_answer {
            return None;
        }
        self.served += 1;

        let mut out = IpcResponderDevice::expected_response(req);
        if self.bugs.wrong_correlation {
            out.id = req.id.wrapping_add(1);
        }
        if self.bugs.wrong_payload {
            for b in out.buf[..out.len as usize].iter_mut() {
                *b = !*b;
            }
        }
        Some(out)
    }

    fn on_peer_gone(&mut self) {
        self.alive = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conforming_responder_echoes_incremented_under_same_id() {
        let mut d = IpcResponderDevice::new();
        let req = Datagram::new(7, &[1, 2, 3]);
        let resp = d.on_request(&req).unwrap();
        assert_eq!(resp.id, 7);
        assert_eq!(resp.payload(), &[2, 3, 4]);
        assert_eq!(d.served(), 1);
    }

    #[test]
    fn wrong_correlation_bug_shifts_the_id() {
        let mut d = IpcResponderDevice::with_bugs(IpcResponderBugs {
            wrong_correlation: true,
            ..Default::default()
        });
        let resp = d.on_request(&Datagram::new(7, &[1])).unwrap();
        assert_eq!(resp.id, 8);
    }

    #[test]
    fn never_answer_bug_returns_nothing() {
        let mut d = IpcResponderDevice::with_bugs(IpcResponderBugs {
            never_answer: true,
            ..Default::default()
        });
        assert!(d.on_request(&Datagram::new(7, &[1])).is_none());
        assert_eq!(d.served(), 0);
    }

    #[test]
    fn wrong_payload_bug_inverts_bytes_but_keeps_the_id() {
        let mut d = IpcResponderDevice::with_bugs(IpcResponderBugs {
            wrong_payload: true,
            ..Default::default()
        });
        let resp = d.on_request(&Datagram::new(7, &[1, 2])).unwrap();
        assert_eq!(resp.id, 7, "a data bug must not disturb correlation");
        assert_eq!(resp.payload(), &[!2u8, !3u8]);
    }

    #[test]
    fn peer_gone_marks_the_responder_dead() {
        let mut d = IpcResponderDevice::new();
        assert!(d.alive());
        d.on_peer_gone();
        assert!(!d.alive());
    }
}
