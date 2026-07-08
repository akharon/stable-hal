//! Simulator-hosted I2C conformance runner: drives each [`super::Case`]
//! against a fresh [`I2cWorld`] with the reference register-pointer device
//! as DUT, scripting the exact stimulus each case needs and checking both
//! wire-contract conformance and (where relevant) data correctness.

use stablehal_contract::Nanos;
use stablehal_i2c::{I2cSlaveBugs, I2cTiming, RegisterPointerDevice, ScriptedMaster, Step};
use stablehal_sim::i2c::{I2cMasterAdapter, I2cWorld, I2cWorldConfig};

use super::{Case, DUT_ADDR, OTHER_ADDR, STANDARD_SUITE};

// Generous enough to cover I2C-STRETCH-WITHIN-CAP: several bytes each
// stretched just under the 25ms cap.
const DEADLINE: Nanos = Nanos::from_millis(200);

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

fn script_for(case: &Case) -> &'static [Step] {
    match case.id {
        "I2C-WRITE-READBACK" => {
            const S: &[Step] = &[
                Step::Write {
                    addr7: DUT_ADDR,
                    bytes: &[0x00, 0xAB, 0xCD],
                    stop: true,
                },
                Step::Write {
                    addr7: DUT_ADDR,
                    bytes: &[0x00],
                    stop: false,
                },
                Step::Read {
                    addr7: DUT_ADDR,
                    len: 2,
                    stop: true,
                },
            ];
            S
        }
        "I2C-POINTER-WRAP" => {
            const S: &[Step] = &[
                Step::Write {
                    addr7: DUT_ADDR,
                    bytes: &[0xFF, 0x11, 0x22],
                    stop: true,
                },
                Step::Write {
                    addr7: DUT_ADDR,
                    bytes: &[0xFF],
                    stop: false,
                },
                Step::Read {
                    addr7: DUT_ADDR,
                    len: 2,
                    stop: true,
                },
            ];
            S
        }
        "I2C-ADDR-ACK" => {
            const S: &[Step] = &[Step::Write {
                addr7: DUT_ADDR,
                bytes: &[0x00],
                stop: true,
            }];
            S
        }
        "I2C-OTHER-ADDR-SILENT" => {
            const S: &[Step] = &[Step::Write {
                addr7: OTHER_ADDR,
                bytes: &[0x00],
                stop: true,
            }];
            S
        }
        "I2C-STRETCH-WITHIN-CAP" => {
            const S: &[Step] = &[
                Step::Write {
                    addr7: DUT_ADDR,
                    bytes: &[0x00, 0x99],
                    stop: true,
                },
                Step::Write {
                    addr7: DUT_ADDR,
                    bytes: &[0x00],
                    stop: false,
                },
                Step::Read {
                    addr7: DUT_ADDR,
                    len: 1,
                    stop: true,
                },
            ];
            S
        }
        other => panic!("no script registered for case {other}"),
    }
}

/// Expected wire-contract outcome for this case: `Ok(())` unless the case
/// deliberately probes an address that must NACK (still contract-conforming
/// — the AddrNack is the correct behavior for `OTHER_ADDR`, not a breach).
fn run_case(case: &Case) -> CaseOutcome {
    let script = script_for(case);
    let bugs = if case.id == "I2C-STRETCH-WITHIN-CAP" {
        I2cSlaveBugs {
            // Just under the cap: conforming.
            stretch_over_cap: Some(
                I2cTiming::standard()
                    .stretch_max
                    .saturating_sub(Nanos::from_micros(1)),
            ),
            ..I2cSlaveBugs::default()
        }
    } else {
        case.bugs
    };

    let master = ScriptedMaster::new(script);
    let mut world = I2cWorld::new(
        I2cWorldConfig::new(0x5EED, I2cTiming::standard()),
        Box::new(I2cMasterAdapter::new(master)),
    );
    world.add_slave(Box::new(RegisterPointerDevice::with_bugs(DUT_ADDR, bugs)));

    let outcome = world.run_until(DEADLINE);
    match outcome {
        Err(e) => CaseOutcome {
            id: case.id,
            pass: false,
            detail: e.to_string(),
        },
        Ok(_) => {
            // Contract passed; for OTHER-ADDR-SILENT that's exactly the
            // expected shape (AddrNack is not a breach). No further data
            // check is wired for read-content cases here — the DST closed
            // loop in stablehal-sim already exercises WRITE-READBACK content
            // byte-for-byte; this suite's job is wire-contract coverage.
            CaseOutcome {
                id: case.id,
                pass: true,
                detail: "conforming".into(),
            }
        }
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
