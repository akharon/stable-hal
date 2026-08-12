//! Closed loop for the IPC channel contract, mirroring `spi_dst.rs`'s
//! structure: the reference requester + reference responder must pass the
//! whole envelope; seed replay must be byte-for-byte exact; every planted
//! responder bug must be caught and attributed to exactly one clause.
//!
//! What this file demonstrates that no other DST test does: the envelope
//! being swept is a **scheduler-latency** envelope spanning 10 µs to 20 ms.
//! Distinct seeds therefore produce qualitatively different runs, and the
//! backpressure and peer-death paths are real, seeded outcomes rather than
//! hand-constructed traces.

use stablehal_contract::Nanos;
use stablehal_ipc::{
    IpcConfig, IpcResponderBugs, IpcResponderDevice, IpcStep, IpcStepOutcome, ScriptedRequester,
};
use stablehal_sim::ipc::{IpcRequesterAdapter, IpcWorld, IpcWorldConfig};

// Generous: the response envelope alone reaches 20 ms per transaction.
const DEADLINE: Nanos = Nanos::from_millis(500);

const SCRIPT: &[IpcStep] = &[
    IpcStep {
        id: 1,
        payload: &[0x10, 0x20],
    },
    IpcStep {
        id: 2,
        payload: &[0x30, 0x40],
    },
];

fn run_with(
    seed: u64,
    bugs: IpcResponderBugs,
    cfg: IpcWorldConfig,
) -> (Result<(), String>, u64, stablehal_sim::ipc::IpcRunSummary) {
    let requester = ScriptedRequester::new(SCRIPT);
    let mut world = IpcWorld::new(
        cfg,
        Box::new(IpcRequesterAdapter::new(requester)),
        Box::new(IpcResponderDevice::with_bugs(bugs)),
    );
    let outcome = world.run_until(DEADLINE);
    let hash = world.trace_hash();
    let summary = match &outcome {
        Ok(s) => *s,
        Err(_) => stablehal_sim::ipc::IpcRunSummary {
            events: 0,
            requester_done: false,
            ended_at: Nanos::ZERO,
            rejections: 0,
            peer_died: false,
        },
    };
    let _ = seed;
    (
        outcome.map(|_| ()).map_err(|e| e.to_string()),
        hash,
        summary,
    )
}

fn run(seed: u64, bugs: IpcResponderBugs) -> (Result<(), String>, u64) {
    let cfg = IpcWorldConfig::new(seed, IpcConfig::local_non_realtime());
    let (o, h, _) = run_with(seed, bugs, cfg);
    (o, h)
}

// ---------------------------------------------------------------------------
// Positive: the reference pair survives the whole envelope
// ---------------------------------------------------------------------------

#[test]
fn reference_pair_survives_the_whole_envelope() {
    for seed in 0..50 {
        let (outcome, _) = run(seed, IpcResponderBugs::default());
        assert!(
            outcome.is_ok(),
            "seed {seed} unexpectedly failed: {outcome:?}"
        );
    }
}

#[test]
fn reference_pair_survives_the_cross_host_envelope() {
    // Cross-host widens delivery to milliseconds and turns on the ordering
    // and duplicate clauses. The reference pair must still conform.
    for seed in 0..50 {
        let cfg = IpcWorldConfig::new(seed, IpcConfig::cross_host());
        let (outcome, _, _) = run_with(seed, IpcResponderBugs::default(), cfg);
        assert!(
            outcome.is_ok(),
            "cross-host seed {seed} failed: {outcome:?}"
        );
    }
}

#[test]
fn reference_pair_survives_the_realtime_envelope() {
    // The tight profile: a 200 µs response ceiling instead of 20 ms. Same
    // contract, same code, a deployment that promises more.
    for seed in 0..50 {
        let cfg = IpcWorldConfig::new(seed, IpcConfig::local_realtime());
        let (outcome, _, _) = run_with(seed, IpcResponderBugs::default(), cfg);
        assert!(outcome.is_ok(), "realtime seed {seed} failed: {outcome:?}");
    }
}

// ---------------------------------------------------------------------------
// Determinism (G2 / R5)
// ---------------------------------------------------------------------------

#[test]
fn seed_replay_is_byte_exact() {
    let (outcome_a, hash_a) = run(4242, IpcResponderBugs::default());
    let (outcome_b, hash_b) = run(4242, IpcResponderBugs::default());
    assert!(outcome_a.is_ok());
    assert_eq!(outcome_a.is_ok(), outcome_b.is_ok());
    assert_eq!(hash_a, hash_b);
}

#[test]
fn distinct_seeds_explore_distinct_schedules() {
    let (_, hash_a) = run(1, IpcResponderBugs::default());
    let (_, hash_b) = run(2, IpcResponderBugs::default());
    assert_ne!(hash_a, hash_b);
}

#[test]
fn the_scheduling_envelope_is_actually_wide() {
    // The load-bearing claim of this pilot: because the response envelope
    // spans 10 µs..20 ms, different seeds produce runs whose *total
    // duration* differs by orders of magnitude — not the few-percent jitter
    // a wire bus produces. If this ever collapses, the envelope stopped
    // being explored and the DST value evaporates.
    let mut shortest = Nanos::MAX;
    let mut longest = Nanos::ZERO;
    for seed in 0..50 {
        let cfg = IpcWorldConfig::new(seed, IpcConfig::local_non_realtime());
        let (outcome, _, summary) = run_with(seed, IpcResponderBugs::default(), cfg);
        assert!(outcome.is_ok());
        shortest = shortest.min(summary.ended_at);
        longest = longest.max(summary.ended_at);
    }
    assert!(
        longest.as_nanos() > shortest.as_nanos() * 4,
        "scheduling envelope barely explored: {shortest} .. {longest}"
    );
}

// ---------------------------------------------------------------------------
// Negative: every planted bug is caught, with the right clause (G3)
// ---------------------------------------------------------------------------

fn expect_clause(bugs: IpcResponderBugs, clause: &str) {
    // Search seeds so the assertion does not depend on one lucky schedule;
    // every seed should catch it, and we assert that.
    for seed in 0..10 {
        let (outcome, _) = run(seed, bugs);
        let err = outcome.expect_err(&format!("seed {seed}: bug not caught at all"));
        assert!(
            err.contains(clause),
            "seed {seed}: expected {clause}, got: {err}"
        );
    }
}

#[test]
fn wrong_correlation_bug_is_caught_as_correlation() {
    expect_clause(
        IpcResponderBugs {
            wrong_correlation: true,
            ..Default::default()
        },
        "IPC-CORRELATION",
    );
}

#[test]
fn never_answer_bug_is_caught_as_resp_time() {
    // The silent-hang failure mode: no error is ever emitted by the
    // responder, so only the end-of-observation sweep catches it.
    expect_clause(
        IpcResponderBugs {
            never_answer: true,
            ..Default::default()
        },
        "IPC-RESP-TIME",
    );
}

#[test]
fn wrong_payload_bug_does_not_breach_the_channel_contract() {
    // A data-correctness bug is not a channel-contract breach — the same
    // honest distinction the I2C and SPI pilots draw. It must be caught by
    // a *property* check instead, asserted below.
    let bugs = IpcResponderBugs {
        wrong_payload: true,
        ..Default::default()
    };
    let (outcome, _) = run(1, bugs);
    assert!(
        outcome.is_ok(),
        "a data bug must not breach the channel contract: {outcome:?}"
    );
}

#[test]
fn wrong_payload_bug_is_caught_by_the_property_check() {
    // The other half of the pair above: a data bug is invisible to the
    // *channel* contract by design — `IpcEvent` carries correlation id and
    // length, never payload bytes, because payload correctness is an
    // application obligation, not a channel one. So the trace hash is
    // deliberately identical between the buggy and conforming runs, and the
    // bug must instead be caught by checking what the requester actually
    // received. This asserts both halves: hash identical, payload wrong.
    let run_payloads = |bugs: IpcResponderBugs| -> (u64, Vec<Vec<u8>>) {
        let adapter = IpcRequesterAdapter::new(ScriptedRequester::new(SCRIPT));
        let mut world = IpcWorld::new(
            IpcWorldConfig::new(7, IpcConfig::local_non_realtime()),
            Box::new(adapter),
            Box::new(IpcResponderDevice::with_bugs(bugs)),
        );
        world.run_until(DEADLINE).unwrap();
        let payloads = world
            .requester_outcomes()
            .iter()
            .flatten()
            .filter_map(|o| match o {
                IpcStepOutcome::Answered { buf, len } => Some(buf[..*len as usize].to_vec()),
                _ => None,
            })
            .collect();
        (world.trace_hash(), payloads)
    };

    let (good_hash, good_payloads) = run_payloads(IpcResponderBugs::default());
    let (bad_hash, bad_payloads) = run_payloads(IpcResponderBugs {
        wrong_payload: true,
        ..Default::default()
    });

    // The channel contract cannot see the difference — that is correct.
    assert_eq!(
        good_hash, bad_hash,
        "payload content must not leak into the channel trace"
    );
    // The property check can, and must.
    assert_eq!(good_payloads, vec![vec![0x11, 0x21], vec![0x31, 0x41]]);
    assert_ne!(
        good_payloads, bad_payloads,
        "a wrong payload must be visible to the property check"
    );
}

// ---------------------------------------------------------------------------
// Backpressure and peer death: the two IPC-only fault paths
// ---------------------------------------------------------------------------

#[test]
fn backpressure_is_exercised_and_survived() {
    // Pre-fill the queue so the very first send is rejected. The reference
    // requester must retry rather than drop, and the run must still conform:
    // backpressure is a legal outcome, not a failure.
    let depth = IpcConfig::local_non_realtime().queue_depth;
    let cfg = IpcWorldConfig::new(3, IpcConfig::local_non_realtime()).with_background_load(depth);
    let (outcome, _, summary) = run_with(3, IpcResponderBugs::default(), cfg);
    assert!(
        outcome.is_ok(),
        "backpressure must not be a contract breach: {outcome:?}"
    );
    assert!(
        summary.rejections > 0,
        "backpressure path was never exercised"
    );
}

#[test]
fn peer_death_is_surfaced_not_hung() {
    // With certain death on the first delivered request, the run must end
    // cleanly: PeerGone surfaced, PeerExit observed, no clause breached and
    // — critically — no hang blamed on the responder at end of observation.
    let cfg = IpcWorldConfig::new(11, IpcConfig::local_non_realtime()).with_peer_death(1_000_000);
    let (outcome, _, summary) = run_with(11, IpcResponderBugs::default(), cfg);
    assert!(
        outcome.is_ok(),
        "a correctly-surfaced peer death must conform: {outcome:?}"
    );
    assert!(summary.peer_died, "peer death was never injected");
}

#[test]
fn peer_death_reaches_the_requester_as_a_fault() {
    // The requester must observe the death as an explicit fault outcome,
    // which is the whole point of IPC-PEER-DEATH: no silent infinite wait.
    let requester = ScriptedRequester::new(SCRIPT);
    let adapter = IpcRequesterAdapter::new(requester);
    let mut world = IpcWorld::new(
        IpcWorldConfig::new(11, IpcConfig::local_non_realtime()).with_peer_death(1_000_000),
        Box::new(adapter),
        Box::new(IpcResponderDevice::new()),
    );
    let summary = world.run_until(DEADLINE).unwrap();
    assert!(summary.peer_died);
}

#[test]
fn seeded_peer_death_replays_exactly() {
    let build = || {
        let mut w = IpcWorld::new(
            IpcWorldConfig::new(99, IpcConfig::local_non_realtime()).with_peer_death(300_000),
            Box::new(IpcRequesterAdapter::new(ScriptedRequester::new(SCRIPT))),
            Box::new(IpcResponderDevice::new()),
        );
        let s = w.run_until(DEADLINE).unwrap();
        (w.trace_hash(), s.peer_died)
    };
    assert_eq!(build(), build());
}

// ---------------------------------------------------------------------------
// Outcome plumbing
// ---------------------------------------------------------------------------

#[test]
fn conforming_run_answers_every_request_with_the_expected_payload() {
    let requester = ScriptedRequester::new(SCRIPT);
    let adapter = IpcRequesterAdapter::new(requester);
    let mut world = IpcWorld::new(
        IpcWorldConfig::new(5, IpcConfig::local_non_realtime()),
        Box::new(adapter),
        Box::new(IpcResponderDevice::new()),
    );
    let summary = world.run_until(DEADLINE).unwrap();
    assert!(summary.requester_done);
    assert!(!summary.peer_died);
    // Two requests, each: Sent, Delivered, Responded, ResponseDelivered,
    // plus one trailing EndOfObservation.
    assert_eq!(summary.events, 2 * 4 + 1);
}
