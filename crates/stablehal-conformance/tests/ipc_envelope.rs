//! Envelope containment for the IPC contract (ADR-11), mirroring
//! `spi_envelope.rs`.
//!
//! Containment carries more weight here than on any wire pilot. A wire bus's
//! timing hull is narrow and predictable, so "did the sim cover the hardware
//! region?" is nearly always yes. A non-real-time IPC channel's hull spans
//! orders of magnitude and depends on machine load, so the question is
//! genuinely open — and answering it is the difference between "our tests
//! pass" and "our tests pass *where production actually runs*".

use stablehal_conformance::ipc::runner::{sim_envelope, sim_envelope_sweep};
use stablehal_contract::{ClauseHull, EnvelopeAccumulator, Nanos};
use stablehal_ipc::IpcConfig;

#[test]
fn reference_run_conforms_and_measures_both_timing_clauses() {
    let (result, acc) = sim_envelope(0x5EED);
    assert!(result.is_ok(), "reference run failed: {result:?}");

    for clause in ["IPC-DELIVERY", "IPC-RESP-TIME"] {
        let hull = acc
            .hull_for(clause)
            .unwrap_or_else(|| panic!("no hull recorded for {clause}"));
        assert!(hull.samples > 0, "{clause} hull is empty");
    }
}

#[test]
fn measured_hulls_sit_inside_the_declared_envelopes() {
    // The measurements a contract reports must be consistent with the
    // bounds it judges against — otherwise the side channel has drifted
    // from the judging path, which is precisely what ADR-11 forbids.
    let cfg = IpcConfig::local_non_realtime();
    let (result, acc) = sim_envelope_sweep(50);
    assert!(result.is_ok(), "sweep failed: {result:?}");

    let delivery = acc.hull_for("IPC-DELIVERY").unwrap();
    assert!(
        delivery.min >= cfg.delivery.min && delivery.max <= cfg.delivery.max,
        "delivery hull {delivery} escaped its declared envelope {:?}",
        cfg.delivery
    );

    let resp = acc.hull_for("IPC-RESP-TIME").unwrap();
    assert!(
        resp.min >= cfg.resp_time.min && resp.max <= cfg.resp_time.max,
        "resp-time hull {resp} escaped its declared envelope {:?}",
        cfg.resp_time
    );
}

#[test]
fn a_sweep_explores_a_wide_scheduling_hull() {
    // The containment check is only meaningful if the sweep actually
    // explores the envelope. Assert the response hull spans at least an
    // order of magnitude — the property that distinguishes this pilot from
    // the wire ones.
    let (result, acc) = sim_envelope_sweep(50);
    assert!(result.is_ok());
    let resp = acc.hull_for("IPC-RESP-TIME").unwrap();
    assert!(
        resp.max.as_nanos() > resp.min.as_nanos() * 10,
        "scheduling hull too narrow to be worth containing: {resp}"
    );
}

#[test]
fn a_sweep_hull_contains_a_single_seeds_hull() {
    // Basic containment sanity: one run's region must sit inside the swept
    // region. If this ever fails, `merge` is broken.
    let (_, one) = sim_envelope(7);
    let (_, swept) = sim_envelope_sweep(50);
    for clause in ["IPC-DELIVERY", "IPC-RESP-TIME"] {
        let a = one.hull_for(clause).unwrap();
        let b = swept.hull_for(clause).unwrap();
        assert!(
            a.contained_in(b),
            "{clause}: single-seed hull {a} not contained in swept hull {b}"
        );
    }
}

#[test]
fn a_production_hull_outside_the_swept_region_is_detected() {
    // The check must have teeth: a "captured" hull whose latencies run
    // slower than anything the sweep explored — still contract-conforming,
    // but untested territory — must be reported as *not* contained.
    // This is the real-world case: production runs on a loaded box, CI does
    // not, and both pass their own contract separately.
    let (_, swept) = sim_envelope_sweep(50);
    let sim_resp = swept.hull_for("IPC-RESP-TIME").unwrap();

    let mut production: EnvelopeAccumulator<4> = EnvelopeAccumulator::new();
    let above = Nanos::from_nanos(sim_resp.max.as_nanos() + 1_000);
    production.observe(stablehal_contract::Measurement {
        clause_id: "IPC-RESP-TIME",
        value: above,
    });
    let prod_hull = production.hull_for("IPC-RESP-TIME").unwrap();

    assert!(
        !prod_hull.contained_in(sim_resp),
        "a production hull beyond the swept region must not be reported contained"
    );
}

#[test]
fn an_empty_hull_is_vacuously_contained() {
    // Guards the honest reading of "UNPROBED": no samples means no claim,
    // not a pass.
    let (_, swept) = sim_envelope_sweep(5);
    let sim_resp = swept.hull_for("IPC-RESP-TIME").unwrap();
    let empty = ClauseHull {
        clause_id: "IPC-RESP-TIME",
        min: Nanos::ZERO,
        max: Nanos::ZERO,
        samples: 0,
    };
    assert!(empty.contained_in(sim_resp));
}
