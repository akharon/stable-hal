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

    #[cfg(feature = "conformance")]
    pub use stablehal_conformance::{
        pattern, runner::run_suite, runner::SuiteConfig, DUT_UNIT, STANDARD_SUITE,
    };
}
