//! Simulator-hosted IPC conformance runner: drives each [`super::Case`]
//! against a fresh [`IpcWorld`] with the reference responder as DUT, and
//! checks the outcome against the case's declared expectation.
//!
//! Also hosts `sim_envelope` (ADR-11), which folds the two scheduler-driven
//! timing clauses into a hull. Containment matters more here than for any
//! wire pilot: the question "did we simulate the latency region production
//! actually runs in?" is the whole risk with non-real-time IPC, and a
//! min/max hull answers it directly.

use stablehal_contract::oracle::check_trace_measured;
use stablehal_contract::{EnvelopeAccumulator, Nanos, Verdict};
use stablehal_ipc::contract::IpcContract;
use stablehal_ipc::{IpcConfig, IpcResponderDevice, IpcStep, ScriptedRequester};
use stablehal_sim::ipc::{IpcRequesterAdapter, IpcWorld, IpcWorldConfig};

use super::{Case, STANDARD_SUITE};

// The scheduling envelope alone reaches 20 ms per exchange.
const DEADLINE: Nanos = Nanos::from_millis(500);

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

fn script_for(case: &Case) -> &'static [IpcStep] {
    match case.id {
        "IPC-BASIC-EXCHANGE" | "IPC-WRONG-CORRELATION" | "IPC-SILENT-HANG" => {
            const S: &[IpcStep] = &[IpcStep {
                id: 1,
                payload: &[0xAB, 0xCD],
            }];
            S
        }
        "IPC-SCHEDULING-ENVELOPE" => {
            const S: &[IpcStep] = &[
                IpcStep {
                    id: 1,
                    payload: &[0x01],
                },
                IpcStep {
                    id: 2,
                    payload: &[0x02],
                },
                IpcStep {
                    id: 3,
                    payload: &[0x03],
                },
                IpcStep {
                    id: 4,
                    payload: &[0x04],
                },
            ];
            S
        }
        "IPC-BACKPRESSURE-RETRY" => {
            const S: &[IpcStep] = &[
                IpcStep {
                    id: 1,
                    payload: &[0x11],
                },
                IpcStep {
                    id: 2,
                    payload: &[0x22],
                },
            ];
            S
        }
        other => panic!("no script registered for case {other}"),
    }
}

fn run_case(case: &Case) -> CaseOutcome {
    let script = script_for(case);
    let requester = ScriptedRequester::new(script);
    let mut cfg = IpcWorldConfig::new(0x5EED, IpcConfig::local_non_realtime());
    if case.background_load > 0 {
        cfg = cfg.with_background_load(case.background_load);
    }
    let mut world = IpcWorld::new(
        cfg,
        Box::new(IpcRequesterAdapter::new(requester)),
        Box::new(IpcResponderDevice::with_bugs(case.bugs)),
    );

    let outcome = world.run_until(DEADLINE);

    match (&outcome, case.expect_breach) {
        // Expected to conform, and did.
        (Ok(_), None) => CaseOutcome {
            id: case.id,
            pass: true,
            detail: "conforming".into(),
        },
        // Expected to conform, but breached.
        (Err(e), None) => CaseOutcome {
            id: case.id,
            pass: false,
            detail: format!("expected conformance, got: {e}"),
        },
        // Expected a breach, and got the right one.
        (Err(e), Some(want)) => {
            let text = e.to_string();
            if text.contains(want) {
                CaseOutcome {
                    id: case.id,
                    pass: true,
                    detail: format!("correctly caught {want}"),
                }
            } else {
                CaseOutcome {
                    id: case.id,
                    pass: false,
                    detail: format!("expected {want}, got: {text}"),
                }
            }
        }
        // Expected a breach, but the run passed — the oracle has no teeth
        // for this case, which is exactly what a suite must surface.
        (Ok(_), Some(want)) => CaseOutcome {
            id: case.id,
            pass: false,
            detail: format!("expected breach of {want}, but the run conformed"),
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

/// Drive the reference requester/responder over a multi-exchange script and
/// fold every timing measurement `IpcContract` reports into an
/// [`EnvelopeAccumulator`] — the sim-explored hull (ADR-11, mirroring
/// `stablehal_conformance::{i2c,lin,spi}::runner::sim_envelope`).
///
/// IPC measures two timing clauses: `IPC-DELIVERY` (send → delivery) and
/// `IPC-RESP-TIME` (delivery → response). Both are transport/scheduler
/// driven, so their hulls are wide — which is the point: comparing a
/// production-captured hull against this one answers whether the simulation
/// explored the latency region the deployment actually occupies.
pub fn sim_envelope(seed: u64) -> (Result<(), String>, EnvelopeAccumulator<4>) {
    const SCRIPT: &[IpcStep] = &[
        IpcStep {
            id: 1,
            payload: &[0xAB, 0xCD],
        },
        IpcStep {
            id: 2,
            payload: &[0x11, 0x22],
        },
        IpcStep {
            id: 3,
            payload: &[0x33, 0x44],
        },
    ];
    let config = IpcConfig::local_non_realtime();

    let requester = ScriptedRequester::new(SCRIPT);
    let mut world = IpcWorld::new(
        IpcWorldConfig::new(seed, config),
        Box::new(IpcRequesterAdapter::new(requester)),
        Box::new(IpcResponderDevice::new()),
    );

    let run_result = world
        .run_until(DEADLINE)
        .map(|_| ())
        .map_err(|e| e.to_string());

    let (verdict, acc) = check_trace_measured(IpcContract::new(config), world.trace());
    let judged = match verdict {
        Verdict::Conforming { .. } => run_result,
        Verdict::Violated(v) => Err(format!("{v}")),
    };
    (judged, acc)
}

/// Fold the hulls from `seeds` separate runs into one — the sim-explored
/// envelope across a sweep, which is what a containment check should be
/// compared against (a single seed explores one point per clause, not a
/// region).
pub fn sim_envelope_sweep(seeds: u64) -> (Result<(), String>, EnvelopeAccumulator<4>) {
    let mut acc: EnvelopeAccumulator<4> = EnvelopeAccumulator::new();
    for seed in 0..seeds {
        let (result, one) = sim_envelope(seed);
        if let Err(e) = result {
            return (Err(format!("seed {seed}: {e}")), acc);
        }
        acc.merge(&one);
    }
    (Ok(()), acc)
}
