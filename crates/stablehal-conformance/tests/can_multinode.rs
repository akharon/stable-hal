//! CAN multi-node topology (risk T8, follow-up): the reference `CanWorld`
//! engine is symmetric and N-node by construction, but until now it was only
//! ever exercised with one driving tester + one passive responder, and its
//! termination rule stopped the whole bus as soon as *any* node was done —
//! wrong for a bus with several contending testers. These tests exercise the
//! richer "stop once every driving node is done" rule and confirm priority
//! arbitration across three simultaneous contenders.

use stablehal_can::contract::CanEvent;
use stablehal_can::node::{CanScriptStep, CanScriptedNode};
use stablehal_can::{CanConfig, CanId};
use stablehal_contract::Nanos;
use stablehal_sim::can::{CanWorld, CanWorldConfig};

const DEADLINE: Nanos = Nanos::from_millis(4);

// Three testers, each offering exactly one frame with a distinct standard
// id, all pending from t=0. None expects a response (`response_id` set to an
// id no node ever sends), so each is `done` the instant its single frame
// wins arbitration and is transmitted.
fn one_shot(id: CanId) -> CanScriptedNode {
    // Leak a single-step script so it satisfies the `&'static` bound without
    // a top-level `static` per id (ids differ per call).
    let script: &'static [CanScriptStep] = Box::leak(Box::new([CanScriptStep {
        id,
        rtr: false,
        data: &[0x00],
    }]));
    // A response id nobody transmits — the tester never advances on a
    // response, only on winning arbitration for its own frame.
    CanScriptedNode::new(script, CanId::Standard(0x7FF))
}

fn transmitted_ids(world: &CanWorld) -> Vec<CanId> {
    world
        .trace()
        .iter()
        .filter_map(|ev| match &ev.event {
            CanEvent::Frame { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

#[test]
fn all_driving_nodes_complete_before_the_bus_stops() {
    // Registration order deliberately NOT priority order, to prove the stop
    // rule waits for every driving node rather than the first/last one.
    let mut world = CanWorld::new(CanWorldConfig::new(0xB0A7, CanConfig::default_500k()));
    world.add_node(Box::new(one_shot(CanId::Standard(0x300))));
    world.add_node(Box::new(one_shot(CanId::Standard(0x100))));
    world.add_node(Box::new(one_shot(CanId::Standard(0x200))));

    world.run_until(DEADLINE).expect("bus conforms");

    let ids = transmitted_ids(&world);
    assert_eq!(ids.len(), 3, "every driving node's frame must be sent");
    // Each distinct id appears exactly once (no node was cut off, none looped).
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 3, "each node transmitted exactly once");
}

#[test]
fn simultaneous_offers_win_in_priority_order() {
    // With all three pending from t=0 and each re-offering until it wins,
    // arbitration must resolve lowest-id-first every round: 0x100, 0x200,
    // 0x300 — regardless of registration order.
    let mut world = CanWorld::new(CanWorldConfig::new(0xB0A7, CanConfig::default_500k()));
    world.add_node(Box::new(one_shot(CanId::Standard(0x300))));
    world.add_node(Box::new(one_shot(CanId::Standard(0x100))));
    world.add_node(Box::new(one_shot(CanId::Standard(0x200))));

    world.run_until(DEADLINE).expect("bus conforms");

    assert_eq!(
        transmitted_ids(&world),
        vec![
            CanId::Standard(0x100),
            CanId::Standard(0x200),
            CanId::Standard(0x300),
        ],
        "lowest CanId must win each arbitration round"
    );
}

#[test]
fn multinode_arbitration_is_deterministic_across_repeated_runs() {
    let build = || {
        let mut world = CanWorld::new(CanWorldConfig::new(0x5EED, CanConfig::default_500k()));
        world.add_node(Box::new(one_shot(CanId::Standard(0x2AB))));
        world.add_node(Box::new(one_shot(CanId::Standard(0x0FF))));
        world.add_node(Box::new(one_shot(CanId::Extended(0x1ABCDE))));
        world.run_until(DEADLINE).expect("bus conforms");
        world
    };
    let a = build();
    let b = build();
    assert_eq!(
        a.trace_hash(),
        b.trace_hash(),
        "same seed => identical trace"
    );
    // Standard ids beat the extended id (CanId Ord ranks Standard first),
    // and lower standard id wins first.
    assert_eq!(
        transmitted_ids(&a),
        vec![
            CanId::Standard(0x0FF),
            CanId::Standard(0x2AB),
            CanId::Extended(0x1ABCDE),
        ],
    );
}
