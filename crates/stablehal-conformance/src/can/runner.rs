//! Simulator-hosted CAN conformance runner: drives each [`super::Case`]
//! against a fresh [`CanWorld`] with the reference responder as DUT,
//! scripting the exact stimulus each case needs and checking wire-contract
//! conformance.

use stablehal_can::node::{CanRegisterNode, CanScriptStep, CanScriptedNode};
use stablehal_can::{CanConfig, CanId};
use stablehal_contract::Nanos;
use stablehal_sim::can::{CanWorld, CanWorldConfig};

use super::{Case, STANDARD_SUITE};

const REQUEST_ID: CanId = CanId::Standard(0x100);
const EXTENDED_REQUEST_ID: CanId = CanId::Extended(0x1ABCDE);
const RESPONSE_ID: CanId = CanId::Standard(0x101);
// Short: each case needs at most one exchange; avoids thousands of idle
// re-poll ticks for cases whose request never draws a response (remote
// frame, extended id — the reference responder only replies to
// `REQUEST_ID`).
const DEADLINE: Nanos = Nanos::from_millis(2);

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

fn script_for(case: &Case) -> &'static [CanScriptStep] {
    match case.id {
        "CAN-DATA-FRAME-BASIC" => {
            const S: &[CanScriptStep] = &[CanScriptStep {
                id: REQUEST_ID,
                rtr: false,
                data: &[0x10],
            }];
            S
        }
        "CAN-REMOTE-FRAME" => {
            const S: &[CanScriptStep] = &[CanScriptStep {
                id: REQUEST_ID,
                rtr: true,
                data: &[],
            }];
            S
        }
        "CAN-EXTENDED-ID" => {
            const S: &[CanScriptStep] = &[CanScriptStep {
                id: EXTENDED_REQUEST_ID,
                rtr: false,
                data: &[0x00],
            }];
            S
        }
        other => panic!("no script registered for case {other}"),
    }
}

fn run_case(case: &Case) -> CaseOutcome {
    let script = script_for(case);
    let mut world = CanWorld::new(CanWorldConfig::new(0x5EED, CanConfig::default_500k()));
    world.add_node(Box::new(CanScriptedNode::new(script, RESPONSE_ID)));
    world.add_node(Box::new(CanRegisterNode::with_bugs(
        REQUEST_ID,
        RESPONSE_ID,
        case.bugs,
    )));

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
