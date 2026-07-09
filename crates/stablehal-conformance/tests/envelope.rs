//! Envelope containment (sim side): `sim_envelope` must judge conforming
//! (the reference master/slave never breach their own contract) and must
//! report a nonempty hull for every timing clause `RtuContract` measures.

use stablehal_conformance::runner::{sim_envelope, SuiteConfig};
use stablehal_modbus::contract::{CL_RTU_RESP_TIME, CL_RTU_T15_INTRA, CL_RTU_T35_FRAME};

#[test]
fn reference_run_conforms_and_measures_every_timing_clause() {
    let cfg = SuiteConfig::default_9600();
    let (result, acc) = sim_envelope(&cfg);
    assert!(result.is_ok(), "reference run must conform: {result:?}");

    // RTU-MASTER-WAIT is not covered here: it only fires when the master
    // starts a new frame while a prior response is still outstanding
    // (retry/timeout path), which a clean back-to-back script never hits.
    for clause in [
        CL_RTU_T35_FRAME.id,
        CL_RTU_T15_INTRA.id,
        CL_RTU_RESP_TIME.id,
    ] {
        let hull = acc.hull_for(clause).unwrap_or_else(|| {
            panic!("expected clause {clause} to have been measured at least once")
        });
        assert!(hull.samples > 0);
        assert!(hull.min <= hull.max);
    }
}

#[test]
fn distinct_seeds_yield_hulls_within_the_configured_envelope() {
    let mut cfg = SuiteConfig::default_9600();
    for seed in [1, 2, 3] {
        cfg.seed = seed;
        let (result, acc) = sim_envelope(&cfg);
        assert!(
            result.is_ok(),
            "seed {seed} unexpectedly failed: {result:?}"
        );
        let t35 = acc.hull_for(CL_RTU_T35_FRAME.id).unwrap();
        assert!(t35.min >= cfg.timing.t35, "seed {seed}: below the floor");
    }
}
