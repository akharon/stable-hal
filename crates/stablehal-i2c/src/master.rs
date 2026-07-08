//! Reference conforming master driver: issues a fixed script of ops against
//! an [`I2cMaster`] port, one at a time, waiting for completion before
//! starting the next. Used both as the DST reference master and as the
//! conformance-suite tester driver (parameterized by its script).

use crate::{I2cBusy, I2cFault, I2cMaster, I2cOp};

/// One scripted step: either write these bytes, or read this many bytes.
#[derive(Copy, Clone, Debug)]
pub enum Step {
    Write {
        addr7: u8,
        bytes: &'static [u8],
        stop: bool,
    },
    Read {
        addr7: u8,
        len: u8,
        stop: bool,
    },
}

/// Outcome of one scripted step, recorded for assertions.
#[derive(Clone, Debug)]
pub enum StepOutcome {
    Wrote,
    Read([u8; 32], u8), // buffer + actual length
    Faulted(I2cFault),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    InFlight,
}

/// Drives `script` to completion, one step at a time, recording outcomes.
pub struct ScriptedMaster {
    script: &'static [Step],
    next: usize,
    phase: Phase,
    pub outcomes: heapless_outcomes::Outcomes,
}

// A tiny fixed-capacity Vec substitute to stay `no_std`-friendly without an
// external dependency (ADR G1): capacity matches the largest conformance
// script used in this crate/suite.
mod heapless_outcomes {
    use super::StepOutcome;

    pub const CAP: usize = 16;

    pub struct Outcomes {
        buf: [Option<StepOutcome>; CAP],
        len: usize,
    }

    impl Outcomes {
        pub const fn new() -> Self {
            Outcomes {
                buf: [const { None }; CAP],
                len: 0,
            }
        }
        pub fn push(&mut self, o: StepOutcome) {
            if self.len < CAP {
                self.buf[self.len] = Some(o);
                self.len += 1;
            }
        }
        pub fn as_slice(&self) -> &[Option<StepOutcome>] {
            &self.buf[..self.len]
        }
        pub fn len(&self) -> usize {
            self.len
        }
        pub fn is_empty(&self) -> bool {
            self.len == 0
        }
    }
}

impl ScriptedMaster {
    pub fn new(script: &'static [Step]) -> Self {
        ScriptedMaster {
            script,
            next: 0,
            phase: Phase::Idle,
            outcomes: heapless_outcomes::Outcomes::new(),
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.script.len()
    }

    pub fn outcomes(&self) -> &[Option<StepOutcome>] {
        self.outcomes.as_slice()
    }

    /// Drive one poll: start the next step if idle, or collect completion.
    /// Returns `true` if progress was made (caller should poll again soon).
    pub fn step(&mut self, port: &mut dyn I2cMaster) -> bool {
        match self.phase {
            Phase::Idle => {
                if self.done() {
                    return false;
                }
                let step = self.script[self.next];
                let op = match step {
                    Step::Write { addr7, bytes, stop } => I2cOp::Write { addr7, bytes, stop },
                    Step::Read { addr7, len, stop } => I2cOp::Read { addr7, len, stop },
                };
                match port.try_begin(&op) {
                    Ok(()) => {
                        self.phase = Phase::InFlight;
                        true
                    }
                    Err(I2cBusy) => false,
                }
            }
            Phase::InFlight => {
                let mut buf = [0u8; 32];
                match port.poll_done(&mut buf) {
                    Some(Ok(())) => {
                        let outcome = match self.script[self.next] {
                            Step::Write { .. } => StepOutcome::Wrote,
                            Step::Read { len, .. } => StepOutcome::Read(buf, len),
                        };
                        self.outcomes.push(outcome);
                        self.next += 1;
                        self.phase = Phase::Idle;
                        true
                    }
                    Some(Err(fault)) => {
                        self.outcomes.push(StepOutcome::Faulted(fault));
                        self.next += 1;
                        self.phase = Phase::Idle;
                        true
                    }
                    None => false,
                }
            }
        }
    }
}
