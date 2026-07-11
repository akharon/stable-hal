//! Simulator-hosted SPI conformance runner: drives each [`super::Case`]
//! against a fresh [`SpiWorld`] with the reference register-pointer device
//! as DUT, scripting the exact stimulus each case needs and checking
//! wire-contract conformance.

use stablehal_contract::Nanos;
use stablehal_sim::spi::{SpiMasterAdapter, SpiWorld, SpiWorldConfig};
use stablehal_spi::{ScriptedSpiMaster, SpiConfig, SpiRegisterDevice, SpiStep};

use super::{Case, DUT_CHIP, STANDARD_SUITE};

// Generous enough to cover SPI-BACK-TO-BACK-TRANSFERS (two transfers).
const DEADLINE: Nanos = Nanos::from_millis(50);

#[derive(Clone, Debug)]
pub struct CaseOutcome {
    pub id: &'static str,
    pub pass: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub outcomes: Vec<CaseOutcome>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.pass).count()
    }
    pub fn all_pass(&self) -> bool {
        self.outcomes.iter().all(|o| o.pass)
    }
    pub fn failure_text(&self) -> String {
        self.outcomes
            .iter()
            .filter(|o| !o.pass)
            .map(|o| format!("[{}] {}", o.id, o.detail))
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn probed_clauses(&self, suite: &[Case]) -> Vec<&'static str> {
        let mut ids: Vec<&'static str> = Vec::new();
        for case in suite {
            for p in case.probes {
                if !ids.contains(p) {
                    ids.push(p);
                }
            }
        }
        ids
    }
}

fn script_for(case: &Case) -> &'static [SpiStep] {
    match case.id {
        "SPI-BASIC-TRANSFER" => {
            const S: &[SpiStep] = &[SpiStep {
                chip: DUT_CHIP,
                tx: &[0x00, 0xAB, 0xCD],
            }];
            S
        }
        "SPI-MULTI-BYTE-TRANSFER" => {
            const S: &[SpiStep] = &[SpiStep {
                chip: DUT_CHIP,
                tx: &[0x00, 1, 2, 3, 4, 5, 6, 7],
            }];
            S
        }
        "SPI-BACK-TO-BACK-TRANSFERS" => {
            const S: &[SpiStep] = &[
                SpiStep {
                    chip: DUT_CHIP,
                    tx: &[0x00, 0x11],
                },
                SpiStep {
                    chip: DUT_CHIP,
                    tx: &[0x00, 0x22],
                },
            ];
            S
        }
        other => panic!("no script registered for case {other}"),
    }
}

fn run_case(case: &Case) -> CaseOutcome {
    let script = script_for(case);
    let master = ScriptedSpiMaster::new(script);
    let mut world = SpiWorld::new(
        SpiWorldConfig::new(0x5EED, SpiConfig::default_1mhz()),
        Box::new(SpiMasterAdapter::new(master)),
    );
    world.add_slave(
        DUT_CHIP,
        Box::new(SpiRegisterDevice::with_bugs(DUT_CHIP, case.bugs)),
    );

    let outcome = world.run_until(DEADLINE);
    match outcome {
        Err(e) => CaseOutcome {
            id: case.id,
            pass: false,
            detail: e.to_string(),
        },
        Ok(_) => CaseOutcome {
            id: case.id,
            pass: true,
            detail: "conforming".into(),
        },
    }
}

pub fn run_suite(suite: &[Case]) -> Report {
    Report {
        outcomes: suite.iter().map(run_case).collect(),
    }
}

pub fn run_standard_suite() -> Report {
    run_suite(STANDARD_SUITE)
}
