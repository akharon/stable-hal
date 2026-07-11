//! Closed loop for the CAN contract (risk T8), mirroring `spi_dst.rs`'s
//! structure but over CAN's broadcast-node topology: a reference tester
//! (`CanScriptedNode`) and reference responder (`CanRegisterNode`) must
//! together conform across the whole envelope; `silent_no_ack` is the one
//! cataloged bug that's wire-contract-visible (breaches `CAN-ACK`, since
//! the responder is the sole other node on this 2-node bus);
//! `wrong_response_data`/`silent_no_response` are non-breaching
//! (data-correctness / application-convention bugs — see
//! `stablehal_can::node::CanNodeBugs`'s doc comment). Seed replay must be
//! byte-for-byte exact.

use stablehal_can::node::{CanNodeBugs, CanRegisterNode, CanScriptStep, CanScriptedNode};
use stablehal_can::{CanConfig, CanId};
use stablehal_contract::Nanos;
use stablehal_sim::can::{CanWorld, CanWorldConfig};

const REQUEST_ID: CanId = CanId::Standard(0x100);
const RESPONSE_ID: CanId = CanId::Standard(0x101);
const DEADLINE: Nanos = Nanos::from_millis(50);

const SCRIPT: &[CanScriptStep] = &[CanScriptStep {
    id: REQUEST_ID,
    rtr: false,
    data: &[0x10],
}];

fn run(seed: u64, bugs: CanNodeBugs) -> (Result<(), String>, u64) {
    let mut world = CanWorld::new(CanWorldConfig::new(seed, CanConfig::default_500k()));
    let mut responder = CanRegisterNode::with_bugs(REQUEST_ID, RESPONSE_ID, bugs);
    responder.write_reg(0x10, 0xAB);
    responder.write_reg(0x11, 0xCD);
    world.add_node(Box::new(CanScriptedNode::new(SCRIPT, RESPONSE_ID)));
    world.add_node(Box::new(responder));
    let outcome = world
        .run_until(DEADLINE)
        .map(|_| ())
        .map_err(|e| e.to_string());
    let hash = world.trace_hash();
    (outcome, hash)
}

#[test]
fn reference_tester_and_responder_survive_the_whole_envelope() {
    for seed in 0..50 {
        let (outcome, _) = run(seed, CanNodeBugs::default());
        assert!(
            outcome.is_ok(),
            "seed {seed} unexpectedly failed: {outcome:?}"
        );
    }
}

#[test]
fn silent_no_ack_is_caught_with_exact_clause() {
    let bugs = CanNodeBugs {
        silent_no_ack: true,
        ..Default::default()
    };
    let (outcome, _) = run(7, bugs);
    let msg = outcome.expect_err("silent_no_ack must be caught");
    assert!(msg.contains("CAN-ACK"), "wrong clause: {msg}");
}

#[test]
fn wrong_response_data_does_not_breach_the_wire_contract() {
    let bugs = CanNodeBugs {
        wrong_response_data: true,
        ..Default::default()
    };
    let (outcome, _) = run(1, bugs);
    assert!(
        outcome.is_ok(),
        "a data-correctness bug must not breach the wire contract: {outcome:?}"
    );
}

#[test]
fn silent_no_response_does_not_breach_the_wire_contract() {
    // The request/response convention is application-level, not part of
    // CanContract, so a responder that never replies still conforms on the
    // wire — it just leaves the tester's schedule un-drained. Run to the
    // deadline directly (bypassing the `n.done()` early-exit) to confirm no
    // contract violation occurs even though the tester's script never
    // completes.
    let mut world =
        stablehal_sim::can::CanWorld::new(CanWorldConfig::new(3, CanConfig::default_500k()));
    let responder = CanRegisterNode::with_bugs(
        REQUEST_ID,
        RESPONSE_ID,
        CanNodeBugs {
            silent_no_response: true,
            ..Default::default()
        },
    );
    world.add_node(Box::new(CanScriptedNode::new(SCRIPT, RESPONSE_ID)));
    world.add_node(Box::new(responder));
    let outcome = world.run_until(DEADLINE);
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn seed_replay_is_byte_exact() {
    let (outcome_a, hash_a) = run(4242, CanNodeBugs::default());
    let (outcome_b, hash_b) = run(4242, CanNodeBugs::default());
    assert!(outcome_a.is_ok());
    assert_eq!(outcome_a.is_ok(), outcome_b.is_ok());
    assert_eq!(hash_a, hash_b);
}

#[test]
fn distinct_seeds_explore_distinct_schedules() {
    let (_, hash_a) = run(1, CanNodeBugs::default());
    let (_, hash_b) = run(2, CanNodeBugs::default());
    assert_ne!(hash_a, hash_b);
}
