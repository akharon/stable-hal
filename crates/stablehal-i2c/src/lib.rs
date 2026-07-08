//! I2C transaction contract: byte/ACK/Start-Stop granularity (UM10204 rev 7),
//! v1 scope is single-master, N-slave. Multi-master arbitration is reserved
//! (clause id `I2C-ARBITRATION`) and explicitly not implemented.
//!
//! Modeled at transaction/byte granularity, not SDA/SCL edges: this is the
//! granularity software contracts actually live at (drivers see bytes and
//! ACKs); edge-level timing is electrical compliance, out of scope — same
//! split as the UART line contract vs. physical layer.

#![no_std]
#![deny(unsafe_code)]

pub mod contract;
pub mod device;
pub mod master;
pub mod timing;

pub use contract::{I2cContract, I2cEvent, I2cState};
pub use device::{I2cSlaveBugs, RegisterPointerDevice};
pub use master::{ScriptedMaster, Step, StepOutcome};
pub use timing::{I2cSpeed, I2cTiming};

use stablehal_contract::Nanos;

/// Raised by [`I2cMaster::try_begin`] when an operation is already in flight.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct I2cBusy;

/// Closed vocabulary of master-observable faults, v1.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum I2cFault {
    /// No slave acknowledged the address byte.
    AddrNack,
    /// Slave NACKed a written data byte (index within this op).
    DataNack { at_byte: u8 },
    /// Cumulative stretching exceeded the contract cap.
    StretchTimeout,
}

/// One I2C bus operation, master-issued.
pub enum I2cOp<'a> {
    /// START, addr+W, bytes...; STOP if `stop`, else leave the bus claimed
    /// (the next op begins with a repeated START).
    Write {
        addr7: u8,
        bytes: &'a [u8],
        stop: bool,
    },
    /// START/reSTART, addr+R, read `len` bytes (master ACKs all but the
    /// last, NACKs the last); STOP if `stop`.
    Read { addr7: u8, len: u8, stop: bool },
}

/// SW-facing HAL trait, poll-driven — same conventions as `UartBus`.
pub trait I2cMaster {
    /// Begin an operation. Err while one is already in flight.
    fn try_begin(&mut self, op: &I2cOp<'_>) -> Result<(), I2cBusy>;
    /// Completion of the in-flight op, if any. Read data (if the op was a
    /// `Read`) is written into `buf`, caller-provided, sized from the op.
    fn poll_done(&mut self, buf: &mut [u8]) -> Option<Result<(), I2cFault>>;
}

/// Byte-level hooks a slave device model implements. The bus (sim or target
/// shim) drives these; timing/ACK mechanics stay in the bus layer.
pub trait I2cSlaveDevice {
    fn addr7(&self) -> u8;
    /// Whether we ACK our own address byte. Default `true`; a device model
    /// planting the `nack_own_addr` bug overrides this to `false`.
    fn ack_addr(&self) -> bool {
        true
    }
    /// Master wrote a byte to us. Return whether we ACK it, and optionally
    /// how long we stretch the clock first (must respect the contract cap).
    fn on_write(&mut self, byte: u8) -> (bool /* ack */, Nanos /* stretch */);
    /// Master reads a byte from us.
    fn on_read(&mut self) -> (u8, Nanos /* stretch */);
    /// Transaction delimiters (for register-pointer state machines).
    fn on_start(&mut self);
    fn on_stop(&mut self);
}
