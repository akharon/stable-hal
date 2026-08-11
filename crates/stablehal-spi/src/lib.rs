//! SPI wire contract: clock-polarity/mode discipline, chip-select timing,
//! mode-fault detection and full-duplex byte exchange. The SPI bus is a
//! single-master, N-slave shared bus (v1). Multi-master is explicitly
//! deferred.
//!
//! Modeled at byte/word granularity with chip-select edges, not individual
//! SCLK edges — this is the granularity at which software contracts live
//! (drivers see bytes and CS assertion). Edge-level timing (SCLK rate,
//! setup/hold times) is expressed as envelopes on the byte transfer.
//!
//! Risk T8 (closed-loop pilot): a conforming reference master
//! ([`master::ScriptedSpiMaster`]) and reference device
//! ([`device::SpiRegisterDevice`]) exist alongside the contract, driven by a
//! `stablehal-sim`-hosted `stablehal_sim::spi::SpiWorld` and a
//! `stablehal_conformance::spi` suite — mirroring MODBUS/I2C/LIN.
//!
//! SPI's fully-synchronous, symmetric byte exchange (every clock both
//! transmits and receives) means CS timing / mode discipline / mode-fault
//! are inherently master-or-world-level obligations: the [`SpiSlave`] trait
//! only ever gets driven while already selected, so it has no hook to
//! misbehave on those clauses the way an I2C/LIN device can withhold an ACK
//! or a checksum. Those clauses' negative-path coverage therefore lives in
//! this crate's `Oracle`-driven unit tests (`contract.rs`), not in a device
//! bug catalog; [`device::SpiSlaveBugs`] covers the one obligation a device
//! *can* control: returning correct read data.
//!
//! # Wire events and clauses
//!
//! | Clause | Citation | Obligation |
//! |--------|----------|------------|
//! | SPI-CS-TIMING | SPI block guide §5.1 | CS must be asserted for the duration of a transfer and de-asserted between transfers (no overlapping CS). |
//! | SPI-MODE-DISC | SPI block guide §5.2 | Master and slave must agree on CPOL and CPHA for the transaction. |
//! | SPI-XFER-TIME | SPI block guide §5.3 | Byte transfer fits within the SCLK-period envelope times 8. |
//! | SPI-DATA-VALID | SPI block guide §5.4 | Master and slave each provide valid data on MISO/MOSI during the active clock phase; no bit-level collisions. Vestigial: no event in this model's granularity carries bit-collision information, so this clause is never checked in `step` (documented, not silently dropped — same honesty pattern as CAN's `CAN-BIT-STUFFING`). |
//! | SPI-MODE-FAULT | SPI block guide §5.5 | A slave must not drive MISO unless its CS is asserted (avoids bus contention). |
//! | SPI-TRANSACTION | — | Every byte belongs to exactly one transaction (between CS assert and CS de-assert). |
//! | SPI-CS-MIN-IDLE | — | Minimum idle time between CS de-assert of one transfer and CS assert of the next (site-configurable). |

#![no_std]
#![deny(unsafe_code)]

pub mod contract;
pub mod device;
pub mod master;

pub use contract::{SpiContract, SpiEvent, SpiState};
pub use device::{SpiRegisterDevice, SpiSlaveBugs};
pub use master::{ScriptedSpiMaster, SpiStep, SpiStepOutcome};

use stablehal_contract::{Nanos, TimeBound};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// SPI clock polarity and phase (the four SPI modes).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SpiMode {
    /// CPOL=0, CPHA=0 — clock idle low, data sampled on rising edge.
    Mode0,
    /// CPOL=0, CPHA=1 — clock idle low, data sampled on falling edge.
    Mode1,
    /// CPOL=1, CPHA=0 — clock idle high, data sampled on falling edge.
    Mode2,
    /// CPOL=1, CPHA=1 — clock idle high, data sampled on rising edge.
    Mode3,
}

impl SpiMode {
    pub const fn cpol(self) -> bool {
        matches!(self, SpiMode::Mode2 | SpiMode::Mode3)
    }
    pub const fn cpha(self) -> bool {
        matches!(self, SpiMode::Mode1 | SpiMode::Mode3)
    }
}

/// Bit order on the wire.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BitOrder {
    MsbFirst,
    LsbFirst,
}

#[derive(Copy, Clone, Debug)]
pub struct SpiConfig {
    /// Nominal SCLK period (e.g. 1 µs for 1 MHz SPI bus).
    pub sclk_period: Nanos,
    /// Allowed deviation of the actual byte-transfer time from
    /// `8 * sclk_period` (clock tolerance, typically ≤ 5%).
    pub sclk_tolerance: Nanos,
    /// Minimum idle time between CS de-assert and the next CS assert
    /// (bus-free guard time).
    pub cs_idle_min: Nanos,
    /// Configured SPI mode for all transactions on this bus.
    pub mode: SpiMode,
    /// Bit order.
    pub bit_order: BitOrder,
}

impl SpiConfig {
    /// A conservative 1 MHz default: 1 µs SCLK period, 5% jitter tolerance,
    /// 100 ns CS idle minimum, CPOL=0 CPHA=0 (mode 0), MSB first.
    pub const fn default_1mhz() -> Self {
        SpiConfig {
            sclk_period: Nanos::from_micros(1),
            sclk_tolerance: Nanos::from_nanos(50), // 5% of 1 µs
            cs_idle_min: Nanos::from_nanos(100),
            mode: SpiMode::Mode0,
            bit_order: BitOrder::MsbFirst,
        }
    }

    /// Envelope for one byte transfer: 8 SCLK cycles ± tolerance.
    pub const fn byte_time_envelope(&self) -> TimeBound {
        let nominal = self.sclk_period.mul_ratio(8, 1);
        TimeBound::new(
            nominal.saturating_sub(self.sclk_tolerance),
            nominal.saturating_add(self.sclk_tolerance),
        )
    }
}

// ---------------------------------------------------------------------------
// SW-facing HAL traits
// ---------------------------------------------------------------------------

/// Error: CS de-asserted mid-transfer (mode fault).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SpiModeFault;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SpiBusy;

/// SW-facing SPI master HAL trait, poll-driven.
pub trait SpiMaster {
    /// Assert CS for `chip`, then begin exchanging `bytes` (MOSI out, MISO in).
    /// `buf` is both source (for TX) and sink (for RX).
    /// Fails if a transfer is already in flight.
    fn try_transfer(&mut self, chip: u8, buf: &mut [u8]) -> Result<(), SpiBusy>;
    /// Poll completion: `Some(Ok(()))` once all bytes transferred and CS
    /// de-asserted; `Some(Err(SpiModeFault))` if CS was lost mid-transfer.
    fn poll_done(&mut self) -> Option<Result<(), SpiModeFault>>;
}

/// SW-facing SPI slave HAL trait, poll-driven.
pub trait SpiSlave {
    /// CS was asserted — prepare for a transfer.
    fn on_select(&mut self, mode: SpiMode);
    /// A byte arrived on MOSI; return the byte to send on MISO.
    fn xfer_byte(&mut self, mosi: u8) -> u8;
    /// CS was de-asserted — transaction over.
    fn on_deselect(&mut self);
}
