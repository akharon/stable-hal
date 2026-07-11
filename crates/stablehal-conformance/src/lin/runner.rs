//! Simulator-hosted LIN conformance runner: drives each [`super::Case`]
//! against a fresh [`LinWorld`] with the reference schedule-table publisher
//! as DUT, scheduling the exact frame each case needs and checking
//! wire-contract conformance.

use stablehal_contract::oracle::check_trace_measured;
use stablehal_contract::{EnvelopeAccumulator, Nanos, Verdict};
use stablehal_lin::{
    LinContract, LinTiming, PublishedFrame, ScheduleEntry, ScheduleMaster, TableSlave,
};
use stablehal_sim::lin::{LinMasterAdapter, LinWorld, LinWorldConfig};

use super::{
    Case, DUT_ID_MAX_DATA, DUT_ID_ONE_BYTE, DUT_ID_TWO_BYTE, STANDARD_SUITE, UNASSIGNED_ID,
};

// Generous enough to cover LIN-MAX-DATA-RESPONSE (8 data bytes + checksum).
const DEADLINE: Nanos = Nanos::from_millis(50);
const BAUD: u32 = 19_200;

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

/// Everything needed to build one case's world: the schedule the tester
/// drives, and the DUT's published frame (`None` for the unassigned-ID case,
/// where no device on the bus owns the scheduled frame at all).
fn case_fixture(case: &Case) -> (&'static [ScheduleEntry], Option<&'static [PublishedFrame]>) {
    match case.id {
        "LIN-BASIC-RESPONSE" => {
            const SCHED: &[ScheduleEntry] = &[ScheduleEntry {
                id: DUT_ID_TWO_BYTE,
                n_data: 2,
            }];
            const FRAMES: &[PublishedFrame] = &[PublishedFrame {
                id: DUT_ID_TWO_BYTE,
                data: [0xAA, 0xBB, 0, 0, 0, 0, 0, 0],
                len: 2,
            }];
            (SCHED, Some(FRAMES))
        }
        "LIN-SINGLE-BYTE-RESPONSE" => {
            const SCHED: &[ScheduleEntry] = &[ScheduleEntry {
                id: DUT_ID_ONE_BYTE,
                n_data: 1,
            }];
            const FRAMES: &[PublishedFrame] = &[PublishedFrame {
                id: DUT_ID_ONE_BYTE,
                data: [0x42, 0, 0, 0, 0, 0, 0, 0],
                len: 1,
            }];
            (SCHED, Some(FRAMES))
        }
        "LIN-MAX-DATA-RESPONSE" => {
            const SCHED: &[ScheduleEntry] = &[ScheduleEntry {
                id: DUT_ID_MAX_DATA,
                n_data: 8,
            }];
            const FRAMES: &[PublishedFrame] = &[PublishedFrame {
                id: DUT_ID_MAX_DATA,
                data: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
                len: 8,
            }];
            (SCHED, Some(FRAMES))
        }
        "LIN-UNASSIGNED-SILENT" => {
            const SCHED: &[ScheduleEntry] = &[ScheduleEntry {
                id: UNASSIGNED_ID,
                n_data: 0,
            }];
            (SCHED, None)
        }
        other => panic!("no fixture registered for case {other}"),
    }
}

fn run_case(case: &Case) -> CaseOutcome {
    let (schedule, frames) = case_fixture(case);
    let master = ScheduleMaster::new(schedule);
    let mut world = LinWorld::new(
        LinWorldConfig::new(0x5EED, LinTiming::new(BAUD)),
        Box::new(LinMasterAdapter::new(master)),
    );
    if let Some(frames) = frames {
        let published: Vec<(u8, u8)> = frames.iter().map(|f| (f.id, f.len)).collect();
        world.add_slave(
            Box::new(TableSlave::with_bugs(frames, case.bugs)),
            &published,
        );
    }

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

/// Run the reference master against the reference publisher for one seed
/// and fold every LIN timing measurement — passing or not — into an
/// [`EnvelopeAccumulator`] (risk T11 / ADR-11, mirroring
/// `stablehal_conformance::runner::sim_envelope` for MODBUS). The schedule
/// runs two frames (2-byte then 8-byte response) so every measured clause
/// (LIN-BREAK, LIN-BREAK-DELIM, LIN-INTERBYTE-MAX, LIN-FRAME-MAXLEN) sees
/// more than one sample.
///
/// Capacity 8 covers the 4 timing clauses `LinContract` currently measures
/// through `step_measured` with headroom.
pub fn sim_envelope(seed: u64) -> (Result<(), String>, EnvelopeAccumulator<8>) {
    const SCHED: &[ScheduleEntry] = &[
        ScheduleEntry {
            id: DUT_ID_TWO_BYTE,
            n_data: 2,
        },
        ScheduleEntry {
            id: DUT_ID_MAX_DATA,
            n_data: 8,
        },
    ];
    const FRAMES: &[PublishedFrame] = &[
        PublishedFrame {
            id: DUT_ID_TWO_BYTE,
            data: [0xAA, 0xBB, 0, 0, 0, 0, 0, 0],
            len: 2,
        },
        PublishedFrame {
            id: DUT_ID_MAX_DATA,
            data: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            len: 8,
        },
    ];
    let timing = LinTiming::new(BAUD);

    let master = ScheduleMaster::new(SCHED);
    let mut world = LinWorld::new(
        LinWorldConfig::new(seed, timing),
        Box::new(LinMasterAdapter::new(master)),
    );
    let published: Vec<(u8, u8)> = FRAMES.iter().map(|f| (f.id, f.len)).collect();
    world.add_slave(Box::new(TableSlave::new(FRAMES)), &published);

    let run_result = world
        .run_until(DEADLINE)
        .map(|_| ())
        .map_err(|e| e.to_string());

    let contract = LinContract::new(timing, &published);
    let (verdict, acc) = check_trace_measured(contract, world.trace());
    let judged = match verdict {
        Verdict::Conforming { .. } => run_result,
        Verdict::Violated(v) => Err(format!("{v}")),
    };
    (judged, acc)
}
