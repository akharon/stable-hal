//! Closed loop for the LIN contract, mirroring `i2c_dst.rs`'s structure:
//! the reference master + reference table slave must pass the whole
//! envelope; each planted `LinSlaveBugs` entry must be caught by the oracle
//! with the exact clause named; seed replay must be byte-for-byte exact.

use stablehal_contract::Nanos;
use stablehal_lin::device::{LinSlaveBugs, PublishedFrame, TableSlave};
use stablehal_lin::master::{ScheduleEntry, ScheduleMaster};
use stablehal_lin::timing::LinTiming;
use stablehal_sim::lin::{LinMasterAdapter, LinWorld, LinWorldConfig};

const FRAME_ID: u8 = 0x10;
const DEADLINE: Nanos = Nanos::from_millis(50);

const SCHEDULE: &[ScheduleEntry] = &[
    ScheduleEntry {
        id: FRAME_ID,
        n_data: 2,
    },
    ScheduleEntry {
        id: FRAME_ID,
        n_data: 2,
    },
];

fn run(seed: u64, bugs: LinSlaveBugs) -> (Result<(), String>, u64) {
    let master = ScheduleMaster::new(SCHEDULE);
    let mut world = LinWorld::new(
        LinWorldConfig::new(seed, LinTiming::new(19_200)),
        Box::new(LinMasterAdapter::new(master)),
    );
    let frame = PublishedFrame {
        id: FRAME_ID,
        data: [0xAA, 0xBB, 0, 0, 0, 0, 0, 0],
        len: 2,
    };
    world.add_slave(
        Box::new(TableSlave::with_bugs(&[frame], bugs)),
        &[(FRAME_ID, 2)],
    );
    let outcome = world
        .run_until(DEADLINE)
        .map(|_| ())
        .map_err(|e| e.to_string());
    let hash = world.trace_hash();
    (outcome, hash)
}

#[test]
fn reference_master_and_slave_survive_the_whole_envelope() {
    for seed in 0..50 {
        let (outcome, _) = run(seed, LinSlaveBugs::default());
        assert!(
            outcome.is_ok(),
            "seed {seed} unexpectedly failed: {outcome:?}"
        );
    }
}

#[test]
fn corrupt_checksum_is_caught_with_exact_clause() {
    let bugs = LinSlaveBugs {
        corrupt_checksum: true,
        ..Default::default()
    };
    let (outcome, _) = run(1, bugs);
    let msg = outcome.expect_err("corrupt_checksum must be caught");
    assert!(msg.contains("LIN-CHECKSUM"), "wrong clause: {msg}");
}

#[test]
fn silent_on_owned_is_caught_with_exact_clause() {
    let bugs = LinSlaveBugs {
        silent_on_owned: true,
        ..Default::default()
    };
    let (outcome, _) = run(2, bugs);
    let msg = outcome.expect_err("silent_on_owned must be caught");
    assert!(msg.contains("LIN-SLAVE-SILENCE"), "wrong clause: {msg}");
}

#[test]
fn seed_replay_is_byte_exact() {
    let (outcome_a, hash_a) = run(4242, LinSlaveBugs::default());
    let (outcome_b, hash_b) = run(4242, LinSlaveBugs::default());
    assert_eq!(outcome_a, outcome_b);
    assert_eq!(hash_a, hash_b, "seed replay diverged — nondeterminism leak");
}

#[test]
fn distinct_seeds_explore_distinct_schedules() {
    let (_, h0) = run(0, LinSlaveBugs::default());
    let (_, h1) = run(1, LinSlaveBugs::default());
    assert_ne!(h0, h1);
}
