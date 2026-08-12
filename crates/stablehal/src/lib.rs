//! One-stop facade over the StableHAL workspace. `use stablehal::prelude::*;`
//! and go — see `docs/getting-started.md` for a full walkthrough.
//!
//! Embedded/`no_std` consumers: `default-features = false` plus whichever
//! protocol features you need (`modbus`, `i2c`, `lin`, `spi`, `can`, `shc1`).
//! `sim` and `conformance` pull in `std` and are meant for host-side testing.
#![cfg_attr(not(feature = "sim"), no_std)]

pub use stablehal_contract as contract;
pub use stablehal_uart as uart;

#[cfg(feature = "can")]
pub use stablehal_can as can;
#[cfg(feature = "conformance")]
pub use stablehal_conformance as conformance;
#[cfg(feature = "i2c")]
pub use stablehal_i2c as i2c;
#[cfg(feature = "ipc")]
pub use stablehal_ipc as ipc;
#[cfg(feature = "lin")]
pub use stablehal_lin as lin;
#[cfg(feature = "modbus")]
pub use stablehal_modbus as modbus;
#[cfg(feature = "shc1")]
pub use stablehal_shc1 as shc1;
#[cfg(feature = "sim")]
pub use stablehal_sim as sim;
#[cfg(feature = "spi")]
pub use stablehal_spi as spi;

/// The types you need for the common paths: writing a protocol driver,
/// running it under DST, and certifying it against the standard suite.
///
/// The kernel (`contract`, `uart`) and the simulator are always present; each
/// protocol's types appear when its feature is enabled, and the matching
/// `stablehal-sim` world types appear when `sim` is on too. Because the
/// default feature set is `sim + modbus + conformance`, a plain
/// `use stablehal::prelude::*;` brings in exactly the MODBUS host path with
/// no name clashes; enabling `i2c`/`lin`/`spi`/`can` adds that protocol's
/// (distinctly named) types alongside.
pub mod prelude {
    pub use stablehal_contract::{
        Breach, Clause, Contract, Nanos, Oracle, SplitMix64, TimeBound, Violation,
    };
    pub use stablehal_uart::{TxBusy, UartBus, UartConfig, UartError};

    #[cfg(feature = "sim")]
    pub use stablehal_sim::{
        Actor, BusObserver, ContractObserver, NextPoll, Sim, SimConfig, UartPort,
    };

    #[cfg(feature = "modbus")]
    pub use stablehal_modbus::{
        MasterConfig, RegisterBank, RtuContract, RtuMaster, RtuSlave, RtuTiming, SlaveConfig,
    };

    #[cfg(feature = "i2c")]
    pub use stablehal_i2c::{
        I2cBusy, I2cContract, I2cEvent, I2cFault, I2cMaster, I2cOp, I2cSlaveBugs, I2cSlaveDevice,
        I2cTiming, RegisterPointerDevice, ScriptedMaster,
    };
    #[cfg(all(feature = "i2c", feature = "sim"))]
    pub use stablehal_sim::i2c::{I2cMasterAdapter, I2cWorld, I2cWorldConfig};

    #[cfg(feature = "ipc")]
    pub use stablehal_ipc::{
        ChannelKind, CorrelationId, Datagram, IpcConfig, IpcContract, IpcEvent, IpcFault,
        IpcRequester, IpcResponder, IpcResponderBugs, IpcResponderDevice, IpcSendError, IpcStep,
        IpcStepOutcome, ScriptedRequester,
    };
    #[cfg(all(feature = "ipc", feature = "sim"))]
    pub use stablehal_sim::ipc::{IpcRequesterAdapter, IpcWorld, IpcWorldConfig};

    #[cfg(feature = "lin")]
    pub use stablehal_lin::{
        LinBus, LinContract, LinEvent, LinSlaveBugs, LinSlaveDevice, LinTiming, PublishedFrame,
        ScheduleEntry, ScheduleMaster, TableSlave,
    };
    #[cfg(all(feature = "lin", feature = "sim"))]
    pub use stablehal_sim::lin::{LinMasterAdapter, LinWorld, LinWorldConfig};

    #[cfg(all(feature = "spi", feature = "sim"))]
    pub use stablehal_sim::spi::{SpiMasterAdapter, SpiWorld, SpiWorldConfig};
    #[cfg(feature = "spi")]
    pub use stablehal_spi::{
        ScriptedSpiMaster, SpiConfig, SpiContract, SpiEvent, SpiMaster, SpiMode, SpiRegisterDevice,
        SpiSlave, SpiSlaveBugs, SpiStep,
    };

    #[cfg(feature = "can")]
    pub use stablehal_can::{
        CanConfig, CanContract, CanEvent, CanFrame, CanId, CanNodeActor, CanNodeBugs,
        CanRegisterNode, CanScriptStep, CanScriptedNode,
    };
    #[cfg(all(feature = "can", feature = "sim"))]
    pub use stablehal_sim::can::{CanWorld, CanWorldConfig};

    #[cfg(feature = "shc1")]
    pub use stablehal_shc1::{parse_stream, write_stream, Header, ParseReport, Record};

    #[cfg(feature = "conformance")]
    pub use stablehal_conformance::{
        pattern, runner::run_suite, runner::SuiteConfig, DUT_UNIT, STANDARD_SUITE,
    };
}
