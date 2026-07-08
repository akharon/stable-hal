//! LIN (Local Interconnect Network) 2.2A transaction contract: frame-field
//! granularity (break / sync / PID / response bytes / checksum), not
//! bit-level UART framing — the same split as the MODBUS RTU contract
//! sitting above the UART line contract. v1 scope: single master, TDMA
//! schedule table, enhanced checksum only (classic checksum is a future
//! revision if a target deployment needs it).
//!
//! LIN differs from both prior pilots in a way worth naming: there is no
//! arbitration (I2C) and no request/response coupling (MODBUS) — the
//! master's sole recurring obligation is "emit a conforming header on
//! schedule"; whichever party (if any) owns the frame ID then has its own,
//! separate obligation to respond correctly. `LinContract` judges both
//! halves independently, which is why `LIN-SLAVE-SILENCE` and
//! `LIN-ONE-RESPONSE` exist as clauses distinct from anything in the
//! MODBUS/I2C clause sets.

#![no_std]
#![deny(unsafe_code)]

pub mod contract;
pub mod device;
pub mod master;
pub mod timing;

pub use contract::{LinContract, LinEvent, LinState};
pub use device::{LinSlaveBugs, PublishedFrame, TableSlave};
pub use master::{LinBus, LinBusy, ScheduleEntry, ScheduleMaster, SlotOutcome};
pub use timing::LinTiming;

/// Closed vocabulary of master-observable faults, v1.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinFault {
    /// Response checksum did not match the data.
    ChecksumMismatch,
    /// Response did not complete within the frame budget.
    FrameTimeout,
}

/// Byte-level hooks a slave device model implements. The bus (sim or target
/// shim) drives this once per frame slot; timing mechanics stay in the bus
/// layer, mirroring `stablehal_i2c::I2cSlaveDevice`'s split.
pub trait LinSlaveDevice {
    /// Given the protected identifier byte and its decoded 6-bit ID,
    /// return `Some((data, len, checksum))` if this device publishes a
    /// response for `id`, or `None` to stay silent (either because it does
    /// not own this ID, or — for bug injection — because it is planted to
    /// withhold a response it should give).
    fn respond(&mut self, pid: u8, id: u8) -> Option<([u8; 8], u8, u8)>;
}
