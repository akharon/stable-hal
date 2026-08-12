//! Reference conforming requester driver: issues a fixed script of requests
//! against an [`IpcRequester`] port, one at a time, awaiting each response
//! before starting the next. Used both as the DST reference requester and as
//! the conformance-suite tester driver (parameterized by its script).
//! Mirrors `stablehal_spi::master::ScriptedSpiMaster`'s shape.
//!
//! The one structurally new behavior versus the wire-bus reference drivers:
//! this driver must **handle backpressure**. A wire master owns the bus and
//! can always transmit; an IPC requester can be told "queue full, try later"
//! at any moment, and a conforming driver retries rather than dropping the
//! datagram (`IPC-BACKPRESSURE`).

use crate::{Datagram, IpcFault, IpcRequester, IpcSendError};

/// One scripted step: send `payload` under correlation id `id`.
#[derive(Copy, Clone, Debug)]
pub struct IpcStep {
    pub id: u32,
    pub payload: &'static [u8],
}

/// Outcome of one scripted step, recorded for assertions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IpcStepOutcome {
    /// Response received. Payload plus its length.
    Answered { buf: [u8; 64], len: u8 },
    /// The channel faulted (peer death, truncation, ...).
    Faulted(IpcFault),
    /// The send was rejected too many times; the driver gave up. An honest
    /// *result*, not a contract violation (§8.3's distinction).
    GaveUp,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    AwaitingResponse,
}

/// How many `WouldBlock` rejections the reference driver tolerates on one
/// step before giving up. Bounded so a permanently-full queue terminates the
/// run instead of spinning forever.
pub const MAX_SEND_ATTEMPTS: u8 = 8;

const CAP: usize = 16;

/// Drives `script` to completion, one request at a time, recording outcomes.
pub struct ScriptedRequester {
    script: &'static [IpcStep],
    next: usize,
    phase: Phase,
    attempts: u8,
    outcomes: [Option<IpcStepOutcome>; CAP],
    outcome_len: usize,
    /// Count of backpressure rejections absorbed across the whole run —
    /// asserted by tests to prove the retry path was actually exercised
    /// rather than merely present.
    rejections: u32,
}

impl ScriptedRequester {
    pub fn new(script: &'static [IpcStep]) -> Self {
        ScriptedRequester {
            script,
            next: 0,
            phase: Phase::Idle,
            attempts: 0,
            outcomes: [None; CAP],
            outcome_len: 0,
            rejections: 0,
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.script.len()
    }

    pub fn outcomes(&self) -> &[Option<IpcStepOutcome>] {
        &self.outcomes[..self.outcome_len]
    }

    /// How many times a send was rejected with `WouldBlock` and retried.
    pub fn rejections(&self) -> u32 {
        self.rejections
    }

    fn push(&mut self, o: IpcStepOutcome) {
        if self.outcome_len < CAP {
            self.outcomes[self.outcome_len] = Some(o);
            self.outcome_len += 1;
        }
    }

    /// Drive one poll: start the next request if idle, or collect a
    /// response. Returns `true` if progress was made (caller should poll
    /// again soon) — same convention as `ScriptedSpiMaster::step`.
    pub fn step(&mut self, port: &mut dyn IpcRequester) -> bool {
        match self.phase {
            Phase::Idle => {
                if self.done() {
                    return false;
                }
                let step = self.script[self.next];
                let msg = Datagram::new(step.id, step.payload);
                match port.try_send(&msg) {
                    Ok(()) => {
                        self.phase = Phase::AwaitingResponse;
                        self.attempts = 0;
                        true
                    }
                    Err(IpcSendError::WouldBlock) => {
                        // Backpressure: legal and recoverable. Retry until
                        // the bounded attempt budget is spent.
                        self.rejections += 1;
                        self.attempts += 1;
                        if self.attempts >= MAX_SEND_ATTEMPTS {
                            self.push(IpcStepOutcome::GaveUp);
                            self.next += 1;
                            self.attempts = 0;
                        }
                        false
                    }
                    Err(IpcSendError::PeerGone) => {
                        self.push(IpcStepOutcome::Faulted(IpcFault::PeerGone));
                        // Terminal: abandon the rest of the script.
                        self.next = self.script.len();
                        true
                    }
                    Err(IpcSendError::TooLong) => {
                        self.push(IpcStepOutcome::GaveUp);
                        self.next += 1;
                        true
                    }
                }
            }
            Phase::AwaitingResponse => match port.try_recv() {
                Some(Ok(msg)) => {
                    let mut buf = [0u8; 64];
                    let len = msg.len.min(64);
                    buf[..len as usize].copy_from_slice(&msg.buf[..len as usize]);
                    self.push(IpcStepOutcome::Answered { buf, len });
                    self.next += 1;
                    self.phase = Phase::Idle;
                    true
                }
                Some(Err(fault)) => {
                    self.push(IpcStepOutcome::Faulted(fault));
                    if fault.is_terminal() {
                        self.next = self.script.len();
                    } else {
                        self.next += 1;
                    }
                    self.phase = Phase::Idle;
                    true
                }
                None => false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port that rejects the first `reject_n` sends, then accepts and
    /// echoes. Exercises the retry path in isolation.
    struct FlakyPort {
        reject_n: u32,
        rejected: u32,
        pending: Option<Datagram>,
    }

    impl IpcRequester for FlakyPort {
        fn try_send(&mut self, msg: &Datagram) -> Result<(), IpcSendError> {
            if self.rejected < self.reject_n {
                self.rejected += 1;
                return Err(IpcSendError::WouldBlock);
            }
            self.pending = Some(*msg);
            Ok(())
        }
        fn try_recv(&mut self) -> Option<Result<Datagram, IpcFault>> {
            self.pending.take().map(Ok)
        }
    }

    const SCRIPT: &[IpcStep] = &[IpcStep {
        id: 1,
        payload: &[0xAA, 0xBB],
    }];

    #[test]
    fn retries_through_backpressure_then_succeeds() {
        let mut r = ScriptedRequester::new(SCRIPT);
        let mut port = FlakyPort {
            reject_n: 3,
            rejected: 0,
            pending: None,
        };
        for _ in 0..32 {
            r.step(&mut port);
        }
        assert!(r.done());
        assert_eq!(r.rejections(), 3, "retry path not exercised");
        assert!(matches!(
            r.outcomes()[0],
            Some(IpcStepOutcome::Answered { .. })
        ));
    }

    #[test]
    fn gives_up_after_the_attempt_budget() {
        let mut r = ScriptedRequester::new(SCRIPT);
        let mut port = FlakyPort {
            reject_n: u32::MAX,
            rejected: 0,
            pending: None,
        };
        for _ in 0..64 {
            r.step(&mut port);
        }
        assert!(r.done());
        assert_eq!(r.outcomes()[0], Some(IpcStepOutcome::GaveUp));
        // Bounded: it stopped at the budget rather than spinning forever.
        assert_eq!(r.rejections(), MAX_SEND_ATTEMPTS as u32);
    }
}
