//! The SPI wire contract: CS timing, mode discipline, and byte-transfer
//! timing, as an executable state machine over byte-level bus events.
//!
//! The same contract instance judges simulated buses and hardware captures.
//! Each party can be excluded from judgement (`judge_master`/`judge_slaves`)
//! so a conformance tester may deliberately emit malformed stimuli while the
//! device under test is still held to every clause.

use stablehal_contract::measure::Measurement;
use stablehal_contract::{Breach, Clause, Contract, Nanos, TimeBound};

use crate::{SpiConfig, SpiMode};

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
        self.step_measured(state, at, event, &mut |_| {})
    }

    /// Envelope-containment side channel (ADR-11): reports every timing
    /// duration it computes — passing or not — before checking the bound, so
    /// `step` and `step_measured` share exactly one judging path. Measures
    /// `SPI-CS-MIN-IDLE` (idle gap between transfers) and `SPI-XFER-TIME`
    /// (per-byte transfer duration).
    fn step_measured(
        &self,
        state: &mut SpiState,
        at: Nanos,
        event: &SpiEvent,
        sink: &mut dyn FnMut(Measurement),
    ) -> Result<(), Breach> {
        match *event {
            SpiEvent::CsAssert { chip, mode } => {
                let phase = core::mem::replace(&mut state.phase, SpiPhase::Idle { since: at });
                match phase {
                    SpiPhase::Idle { since } => {
                        let idle = at.saturating_sub(since);
                        sink(Measurement {
                            clause_id: CL_SPI_CS_MIN_IDLE.id,
                            value: idle,
                        });
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
                        sink(Measurement {
                            clause_id: CL_SPI_XFER_TIME.id,
                            value: elapsed,
                        });
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
