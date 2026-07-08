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
//! # Wire events and clauses
//!
//! | Clause | Citation | Obligation |
//! |--------|----------|------------|
//! | SPI-CS-TIMING | SPI block guide §5.1 | CS must be asserted for the duration of a transfer and de-asserted between transfers (no overlapping CS). |
//! | SPI-MODE-DISC | SPI block guide §5.2 | Master and slave must agree on CPOL and CPHA for the transaction. |
//! | SPI-XFER-TIME | SPI block guide §5.3 | Byte transfer fits within the SCLK-period envelope times 8. |
//! | SPI-DATA-VALID | SPI block guide §5.4 | Master and slave each provide valid data on MISO/MOSI during the active clock phase; no bit-level collisions. |
//! | SPI-MODE-FAULT | SPI block guide §5.5 | A slave must not drive MISO unless its CS is asserted (avoids bus contention). |
//! | SPI-TRANSACTION | — | Every byte belongs to exactly one transaction (between CS assert and CS de-assert). |
//! | SPI-CS-MIN-IDLE | — | Minimum idle time between CS de-assert of one transfer and CS assert of the next (site-configurable). |

#![no_std]
#![deny(unsafe_code)]

use stablehal_contract::{Breach, Clause, Contract, Nanos, TimeBound};

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

// ---------------------------------------------------------------------------
// Wire-facing contract
// ---------------------------------------------------------------------------

/// Observable events on the SPI bus.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SpiEvent {
    /// Master asserted CS for slave `chip` with given mode.
    CsAssert { chip: u8, mode: SpiMode },
    /// One byte exchanged: `mosi` (master→slave), `miso` (slave→master).
    ByteXfer { chip: u8, mosi: u8, miso: u8 },
    /// Master de-asserted CS.
    CsDeAssert { chip: u8 },
    /// Mode fault: a slave drove MISO without CS being asserted, or CS was
    /// lost mid-transfer.
    ModeFault { chip: u8 },
    /// Appended by the harness after all activity: closes any open
    /// transaction.
    EndOfObservation,
}

pub static CL_SPI_CS_TIMING: Clause = Clause {
    id: "SPI-CS-TIMING",
    citation: "SPI block guide §5.1",
    text: "CS must be asserted for the duration of a transfer and de-asserted between transfers; no overlapping CS assertions.",
};

pub static CL_SPI_MODE_DISC: Clause = Clause {
    id: "SPI-MODE-DISC",
    citation: "SPI block guide §5.2",
    text: "Master and slave must agree on CPOL and CPHA for the transaction.",
};

pub static CL_SPI_XFER_TIME: Clause = Clause {
    id: "SPI-XFER-TIME",
    citation: "SPI block guide §5.3",
    text: "Each byte transfer fits within 8× the SCLK period envelope (plus tolerance).",
};

pub static CL_SPI_DATA_VALID: Clause = Clause {
    id: "SPI-DATA-VALID",
    citation: "SPI block guide §5.4",
    text: "Master and slave each provide valid data on MISO/MOSI during the active clock phase; no bit-level collisions.",
};

pub static CL_SPI_MODE_FAULT: Clause = Clause {
    id: "SPI-MODE-FAULT",
    citation: "SPI block guide §5.5",
    text: "A slave must not drive MISO unless its CS is asserted (avoids bus contention).",
};

pub static CL_SPI_TRANSACTION: Clause = Clause {
    id: "SPI-TRANSACTION",
    citation: "SPI protocol convention",
    text: "Every byte belongs to exactly one transaction (between CS assert and CS de-assert).",
};

pub static CL_SPI_CS_MIN_IDLE: Clause = Clause {
    id: "SPI-CS-MIN-IDLE",
    citation: "SPI protocol convention",
    text: "Minimum idle time between CS de-assert of one transfer and CS assert of the next.",
};

static SPI_CLAUSES: [&Clause; 7] = [
    &CL_SPI_CS_TIMING,
    &CL_SPI_MODE_DISC,
    &CL_SPI_XFER_TIME,
    &CL_SPI_DATA_VALID,
    &CL_SPI_MODE_FAULT,
    &CL_SPI_TRANSACTION,
    &CL_SPI_CS_MIN_IDLE,
];

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct SpiContract {
    pub config: SpiConfig,
    /// Judge the master's obligations.
    pub judge_master: bool,
    /// Judge slave obligations.
    pub judge_slaves: bool,
}

impl SpiContract {
    pub fn new(config: SpiConfig) -> Self {
        SpiContract {
            config,
            judge_master: true,
            judge_slaves: true,
        }
    }

    fn fail_master(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_master {
            Err(breach)
        } else {
            Ok(())
        }
    }

    fn fail_slave(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_slaves {
            Err(breach)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
enum SpiPhase {
    Idle {
        since: Nanos,
    },
    Transfer {
        chip: u8,
        #[allow(dead_code)]
        mode: SpiMode,
        byte_count: u16,
        xfer_start: Nanos,
    },
}

#[derive(Clone, Debug)]
pub struct SpiState {
    phase: SpiPhase,
}

impl Contract for SpiContract {
    type State = SpiState;
    type Event = SpiEvent;

    fn initial(&self) -> SpiState {
        SpiState {
            phase: SpiPhase::Idle { since: Nanos::ZERO },
        }
    }

    fn step(&self, state: &mut SpiState, at: Nanos, event: &SpiEvent) -> Result<(), Breach> {
        match *event {
            SpiEvent::CsAssert { chip, mode } => {
                let phase = core::mem::replace(&mut state.phase, SpiPhase::Idle { since: at });
                match phase {
                    SpiPhase::Idle { since } => {
                        let idle = at.saturating_sub(since);
                        if idle < self.config.cs_idle_min {
                            self.fail_master(Breach::timing(
                                &CL_SPI_CS_MIN_IDLE,
                                idle,
                                TimeBound::at_least(self.config.cs_idle_min),
                            ))?;
                        }
                    }
                    SpiPhase::Transfer { chip: _prev, .. } => {
                        // CS assert while another CS was already asserted:
                        // overlapping chip selects.
                        self.fail_master(Breach::of(&CL_SPI_CS_TIMING))?;
                        // Check mode discipline on the new claim.
                        if mode != self.config.mode {
                            self.fail_master(Breach::of(&CL_SPI_MODE_DISC))?;
                        }
                    }
                }
                if mode != self.config.mode {
                    self.fail_master(Breach::of(&CL_SPI_MODE_DISC))?;
                }
                state.phase = SpiPhase::Transfer {
                    chip,
                    mode,
                    byte_count: 0,
                    xfer_start: at,
                };
                Ok(())
            }
            SpiEvent::ByteXfer {
                mosi: _, miso: _, ..
            } => {
                match &mut state.phase {
                    SpiPhase::Transfer {
                        chip: _,
                        mode: _,
                        byte_count,
                        xfer_start,
                    } => {
                        let elapsed = at.saturating_sub(*xfer_start);
                        let expected_dur = self
                            .config
                            .sclk_period
                            .mul_ratio(8 * (*byte_count as u64 + 1), 1);
                        let tolerance = self
                            .config
                            .sclk_tolerance
                            .mul_ratio(*byte_count as u64 + 1, 1);
                        let envelope = TimeBound::new(
                            expected_dur.saturating_sub(tolerance),
                            expected_dur.saturating_add(tolerance),
                        );
                        if !envelope.contains(elapsed) {
                            self.fail_master(Breach::timing(&CL_SPI_XFER_TIME, elapsed, envelope))?;
                        }
                        *byte_count += 1;
                    }
                    SpiPhase::Idle { .. } => {
                        // Byte exchanged outside a transaction.
                        self.fail_master(Breach::of(&CL_SPI_TRANSACTION))?;
                    }
                }
                Ok(())
            }
            SpiEvent::CsDeAssert { chip: _ } => {
                let phase = core::mem::replace(&mut state.phase, SpiPhase::Idle { since: at });
                match phase {
                    SpiPhase::Transfer { byte_count, .. } => {
                        if byte_count == 0 {
                            // CS asserted then de-asserted with no bytes:
                            // runt transaction.
                            self.fail_master(Breach::of(&CL_SPI_TRANSACTION))?;
                        }
                    }
                    SpiPhase::Idle { .. } => {
                        // CS de-assert without a preceding assert.
                        self.fail_master(Breach::of(&CL_SPI_CS_TIMING))?;
                    }
                }
                Ok(())
            }
            SpiEvent::ModeFault { chip: _ } => {
                self.fail_slave(Breach::of(&CL_SPI_MODE_FAULT))?;
                // Reset to idle after a mode fault.
                state.phase = SpiPhase::Idle { since: at };
                Ok(())
            }
            SpiEvent::EndOfObservation => {
                // If a transfer is open, close it.
                if let SpiPhase::Transfer { .. } = &state.phase {
                    state.phase = SpiPhase::Idle { since: at };
                }
                Ok(())
            }
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &SPI_CLAUSES
    }
}

// ---------------------------------------------------------------------------
// Reference slave device: simple register file
// ---------------------------------------------------------------------------

/// A minimal SPI slave: 256 byte register file, addressed by the first byte
/// of the transfer as the register offset; subsequent bytes read/write
/// sequentially with auto-increment.
pub struct SpiRegisterDevice {
    #[allow(dead_code)]
    addr: u8, // chip-select address
    regs: [u8; 256],
    ptr: u8,
    selected: bool,
    byte_idx: u8,
}

impl SpiRegisterDevice {
    pub fn new(addr: u8) -> Self {
        SpiRegisterDevice {
            addr,
            regs: [0u8; 256],
            ptr: 0,
            selected: false,
            byte_idx: 0,
        }
    }

    pub fn read_reg(&self, offset: u8) -> u8 {
        self.regs[offset as usize]
    }

    pub fn write_reg(&mut self, offset: u8, val: u8) {
        self.regs[offset as usize] = val;
    }
}

impl SpiSlave for SpiRegisterDevice {
    fn on_select(&mut self, _mode: SpiMode) {
        self.selected = true;
        self.byte_idx = 0;
    }

    fn xfer_byte(&mut self, mosi: u8) -> u8 {
        if self.byte_idx == 0 {
            // First byte: register pointer.
            self.ptr = mosi;
            self.byte_idx += 1;
            self.regs[self.ptr as usize]
        } else {
            // Subsequent bytes: write then auto-increment read.
            let prev = self.regs[self.ptr as usize];
            self.regs[self.ptr as usize] = mosi;
            self.ptr = self.ptr.wrapping_add(1);
            self.byte_idx += 1;
            prev
        }
    }

    fn on_deselect(&mut self) {
        self.selected = false;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::Oracle;

    fn cfg() -> SpiConfig {
        SpiConfig::default_1mhz()
    }

    #[test]
    fn conforming_transfer_passes() {
        let mut o = Oracle::new(SpiContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let byte_time = cfg().sclk_period.mul_ratio(8, 1);

        o.observe(
            t,
            &SpiEvent::CsAssert {
                chip: 0,
                mode: SpiMode::Mode0,
            },
        )
        .unwrap();
        let t1 = t + byte_time;
        o.observe(
            t1,
            &SpiEvent::ByteXfer {
                chip: 0,
                mosi: 0xAA,
                miso: 0x55,
            },
        )
        .unwrap();
        let t2 = t1 + byte_time;
        o.observe(
            t2,
            &SpiEvent::ByteXfer {
                chip: 0,
                mosi: 0xBB,
                miso: 0x66,
            },
        )
        .unwrap();
        let t3 = t2 + cfg().cs_idle_min;
        o.observe(t3, &SpiEvent::CsDeAssert { chip: 0 }).unwrap();
        o.observe(t3, &SpiEvent::EndOfObservation).unwrap();
    }

    #[test]
    fn byte_outside_transaction_is_transaction_breach() {
        let mut o = Oracle::new(SpiContract::new(cfg()));
        let t = Nanos::from_micros(5);
        let v = o
            .observe(
                t,
                &SpiEvent::ByteXfer {
                    chip: 0,
                    mosi: 0xAA,
                    miso: 0x55,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "SPI-TRANSACTION");
    }

    #[test]
    fn overlapping_cs_is_cs_timing_breach() {
        let mut o = Oracle::new(SpiContract::new(cfg()));
        let t = Nanos::from_micros(10);

        o.observe(
            t,
            &SpiEvent::CsAssert {
                chip: 0,
                mode: SpiMode::Mode0,
            },
        )
        .unwrap();
        // Second CS assert without de-assert in between.
        let v = o
            .observe(
                t + Nanos::from_micros(5),
                &SpiEvent::CsAssert {
                    chip: 1,
                    mode: SpiMode::Mode0,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "SPI-CS-TIMING");
    }

    #[test]
    fn mode_mismatch_breaches_mode_disc() {
        let mut o = Oracle::new(SpiContract::new(cfg()));
        let t = Nanos::from_micros(5);
        let v = o
            .observe(
                t,
                &SpiEvent::CsAssert {
                    chip: 0,
                    mode: SpiMode::Mode3,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "SPI-MODE-DISC");
    }

    #[test]
    fn cs_idle_too_short_breaches_min_idle() {
        let mut o = Oracle::new(SpiContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let byte_time = cfg().sclk_period.mul_ratio(8, 1);

        // First transfer.
        o.observe(
            t,
            &SpiEvent::CsAssert {
                chip: 0,
                mode: SpiMode::Mode0,
            },
        )
        .unwrap();
        let t1 = t + byte_time;
        o.observe(
            t1,
            &SpiEvent::ByteXfer {
                chip: 0,
                mosi: 1,
                miso: 2,
            },
        )
        .unwrap();
        o.observe(t1, &SpiEvent::CsDeAssert { chip: 0 }).unwrap();

        // Second transfer with zero idle — too short.
        let v = o
            .observe(
                t1,
                &SpiEvent::CsAssert {
                    chip: 0,
                    mode: SpiMode::Mode0,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "SPI-CS-MIN-IDLE");
    }

    #[test]
    fn mode_fault_breaches_mode_fault() {
        let mut o = Oracle::new(SpiContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let v = o.observe(t, &SpiEvent::ModeFault { chip: 0 }).unwrap_err();
        assert_eq!(v.breach.clause.id, "SPI-MODE-FAULT");
    }

    #[test]
    fn register_device_basic_write_read() {
        let mut dev = SpiRegisterDevice::new(0);
        dev.write_reg(0x10, 0xAB);

        dev.on_select(SpiMode::Mode0);
        // First byte: set pointer to 0x10, read returns 0xAB.
        assert_eq!(dev.xfer_byte(0x10), 0xAB);
        // Second byte: write 0xCD to regs[0x10], return old value (0xAB).
        assert_eq!(dev.xfer_byte(0xCD), 0xAB);
        // Read back: pointer now at 0x11, write 0xFF, return old value.
        assert_eq!(dev.xfer_byte(0xFF), 0x00);
        dev.on_deselect();

        assert_eq!(dev.read_reg(0x10), 0xCD);
        assert_eq!(dev.read_reg(0x11), 0xFF);
    }

    #[test]
    fn clause_list_is_complete() {
        let c = SpiContract::new(cfg());
        let clauses = c.clauses();
        assert_eq!(clauses.len(), 7);
        let ids: [&str; 7] = [
            "SPI-CS-TIMING",
            "SPI-MODE-DISC",
            "SPI-XFER-TIME",
            "SPI-DATA-VALID",
            "SPI-MODE-FAULT",
            "SPI-TRANSACTION",
            "SPI-CS-MIN-IDLE",
        ];
        for id in &ids {
            assert!(clauses.iter().any(|cl| cl.id == *id), "missing clause {id}");
        }
    }
}
