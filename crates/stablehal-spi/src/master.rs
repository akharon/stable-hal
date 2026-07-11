//! Reference conforming master driver: issues a fixed script of transfers
//! against an [`SpiMaster`] port, one at a time, waiting for completion
//! before starting the next. Used both as the DST reference master and as
//! the conformance-suite tester driver (parameterized by its script).
//! Mirrors `stablehal_i2c::master::ScriptedMaster` exactly.

use crate::{SpiBusy, SpiMaster, SpiModeFault};

/// One scripted step: transfer `tx` bytes with `chip` selected (MOSI out,
/// MISO in — SPI's full-duplex exchange makes read vs. write a single op).
#[derive(Copy, Clone, Debug)]
pub struct SpiStep {
    pub chip: u8,
    pub tx: &'static [u8],
}

/// Outcome of one scripted step, recorded for assertions.
#[derive(Clone, Debug)]
pub enum SpiStepOutcome {
    Transferred([u8; 32], u8), // rx buffer + actual length
    Faulted(SpiModeFault),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    InFlight,
}

const MAX_XFER_BYTES: usize = 32;

/// Drives `script` to completion, one step at a time, recording outcomes.
pub struct ScriptedSpiMaster {
    script: &'static [SpiStep],
    next: usize,
    phase: Phase,
    buf: [u8; MAX_XFER_BYTES],
    len: u8,
    outcomes: heapless_outcomes::Outcomes,
}

// A tiny fixed-capacity Vec substitute to stay `no_std`-friendly without an
// external dependency (ADR G1), mirroring `stablehal_i2c::master`'s.
mod heapless_outcomes {
    use super::SpiStepOutcome;

    pub const CAP: usize = 16;

    pub struct Outcomes {
        buf: [Option<SpiStepOutcome>; CAP],
        len: usize,
    }

    impl Outcomes {
        pub const fn new() -> Self {
            Outcomes {
                buf: [const { None }; CAP],
                len: 0,
            }
        }
        pub fn push(&mut self, o: SpiStepOutcome) {
            if self.len < CAP {
                self.buf[self.len] = Some(o);
                self.len += 1;
            }
        }
        pub fn as_slice(&self) -> &[Option<SpiStepOutcome>] {
            &self.buf[..self.len]
        }
    }
}

impl ScriptedSpiMaster {
    pub fn new(script: &'static [SpiStep]) -> Self {
        ScriptedSpiMaster {
            script,
            next: 0,
            phase: Phase::Idle,
            buf: [0u8; MAX_XFER_BYTES],
            len: 0,
            outcomes: heapless_outcomes::Outcomes::new(),
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.script.len()
    }

    pub fn outcomes(&self) -> &[Option<SpiStepOutcome>] {
        self.outcomes.as_slice()
    }

    /// Drive one poll: start the next step if idle, or collect completion.
    /// Returns `true` if progress was made (caller should poll again soon).
    pub fn step(&mut self, port: &mut dyn SpiMaster) -> bool {
        match self.phase {
            Phase::Idle => {
                if self.done() {
                    return false;
                }
                let step = self.script[self.next];
                let len = step.tx.len().min(MAX_XFER_BYTES);
                self.buf[..len].copy_from_slice(&step.tx[..len]);
                self.len = len as u8;
                match port.try_transfer(step.chip, &mut self.buf[..len]) {
                    Ok(()) => {
                        self.phase = Phase::InFlight;
                        true
                    }
                    Err(SpiBusy) => false,
                }
            }
            Phase::InFlight => match port.poll_done() {
                Some(Ok(())) => {
                    let n = self.len as usize;
                    let mut rx = [0u8; 32];
                    rx[..n].copy_from_slice(&self.buf[..n]);
                    self.outcomes
                        .push(SpiStepOutcome::Transferred(rx, self.len));
                    self.next += 1;
                    self.phase = Phase::Idle;
                    true
                }
                Some(Err(fault)) => {
                    self.outcomes.push(SpiStepOutcome::Faulted(fault));
                    self.next += 1;
                    self.phase = Phase::Idle;
                    true
                }
                None => false,
            },
        }
    }
}
