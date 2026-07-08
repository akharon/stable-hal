//! Reference conforming master: drives a fixed schedule table of frame IDs,
//! one at a time — send header (break/sync/PID), wait out the response
//! window, move to the next slot. Used both as the DST reference master and
//! as the conformance-suite tester driver (parameterized by its schedule).
//!
//! LIN has no request/response coupling at the transport level the way
//! MODBUS does: the master's only obligation per slot is to emit a
//! conforming header on schedule. Whether *this* driver reads a response is
//! a separate concern (`LinMaster::poll_response`) from whether the
//! contract was honored (`LinContract` judges the header/response
//! independent of whether this particular master consumed the bytes).

use crate::contract::encode_pid;
use crate::LinFault;

/// One schedule-table entry: frame ID + how many data bytes its response
/// carries (needed to size the frame timing budget, mirroring how a real
/// LIN cluster description file fixes response length per frame).
#[derive(Copy, Clone, Debug)]
pub struct ScheduleEntry {
    pub id: u8,
    pub n_data: u8,
}

/// Outcome of one scheduled slot.
#[derive(Copy, Clone, Debug)]
pub enum SlotOutcome {
    /// Response received; `len` bytes valid in the buffer, checksum matched.
    Responded {
        len: u8,
    },
    /// No response was driven for this slot (legal for an unassigned ID).
    NoResponse,
    Faulted(LinFault),
}

/// SW-facing HAL trait a target's LIN peripheral (or bit-banged UART, since
/// LIN is UART-compatible at the physical layer) implements. Poll-driven,
/// same conventions as `UartBus`/`I2cMaster`.
pub trait LinBus {
    /// Emit one frame header: break, break delimiter, sync (0x55), PID.
    /// Returns `Busy` while a previous header/response cycle is still in
    /// flight.
    fn try_send_header(&mut self, pid: u8) -> Result<(), LinBusy>;
    /// Poll for the response to the most recent header. `None` while still
    /// waiting (caller must eventually time out per the frame budget);
    /// `Some` once the response window has closed (bytes present, or none).
    fn poll_response(&mut self, buf: &mut [u8]) -> Option<SlotOutcome>;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LinBusy;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    InFlight,
}

const MAX_SCHEDULE: usize = 32;

/// Drives a fixed schedule to completion, one slot at a time, recording
/// outcomes in a fixed-capacity buffer (no allocator, ADR G1).
pub struct ScheduleMaster {
    schedule: &'static [ScheduleEntry],
    next: usize,
    phase: Phase,
    outcomes: [Option<SlotOutcome>; MAX_SCHEDULE],
    outcomes_len: usize,
}

impl ScheduleMaster {
    pub fn new(schedule: &'static [ScheduleEntry]) -> Self {
        ScheduleMaster {
            schedule,
            next: 0,
            phase: Phase::Idle,
            outcomes: [None; MAX_SCHEDULE],
            outcomes_len: 0,
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.schedule.len()
    }

    pub fn outcomes(&self) -> &[Option<SlotOutcome>] {
        &self.outcomes[..self.outcomes_len]
    }

    fn push_outcome(&mut self, o: SlotOutcome) {
        if self.outcomes_len < MAX_SCHEDULE {
            self.outcomes[self.outcomes_len] = Some(o);
            self.outcomes_len += 1;
        }
    }

    /// Drive one poll: send the next slot's header if idle, or collect the
    /// response outcome. Returns `true` if progress was made.
    pub fn step(&mut self, bus: &mut dyn LinBus) -> bool {
        match self.phase {
            Phase::Idle => {
                if self.done() {
                    return false;
                }
                let entry = self.schedule[self.next];
                let pid = encode_pid(entry.id);
                match bus.try_send_header(pid) {
                    Ok(()) => {
                        self.phase = Phase::InFlight;
                        true
                    }
                    Err(LinBusy) => false,
                }
            }
            Phase::InFlight => {
                let mut buf = [0u8; 8];
                match bus.poll_response(&mut buf) {
                    Some(outcome) => {
                        self.push_outcome(outcome);
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
