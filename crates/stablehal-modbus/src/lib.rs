//! MODBUS RTU as an executable contract, plus a contract-conforming master
//! driver and reference slave — both written against the [`stablehal_uart`]
//! HAL traits only, so the same code runs under simulation and on target.
//!
//! Subset implemented (documented scope, fixed for v0.1):
//! * Function codes 0x03 (Read Holding Registers), 0x06 (Write Single
//!   Register), 0x10 (Write Multiple Registers) + exception responses.
//! * Unicast only; broadcast (unit 0) is out of subset.
//!
//! Spec citations refer to *MODBUS over Serial Line Specification and
//! Implementation Guide V1.02* ("SL") and *MODBUS Application Protocol
//! Specification V1.1b3* ("AP").

#![no_std]
#![deny(unsafe_code)]

pub mod contract;
pub mod crc;
pub mod master;
pub mod pdu;
pub mod slave;
pub mod timing;

pub use contract::{Dir, RtuContract, RtuEvent, RtuState};
pub use master::{MasterConfig, MasterError, MasterProgress, RtuMaster};
pub use pdu::{Adu, ExceptionCode, Request, MAX_ADU};
pub use slave::{RegisterBank, RtuSlave, SlaveBugs, SlaveConfig, SlaveProgress};
pub use timing::RtuTiming;
