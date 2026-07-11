//! Envelope containment for LIN (risk T11): `sim_envelope` must judge
//! conforming (the reference master/publisher never breach their own
//! contract) and must report a nonempty hull for every timing clause
//! `LinContract` measures through `step_measured`.

use stablehal_conformance::lin::runner::sim_envelope;
use stablehal_lin::contract::{
    CL_LIN_BREAK, CL_LIN_BREAK_DELIM, CL_LIN_FRAME_MAXLEN, CL_LIN_INTERBYTE_MAX,
};

#[test]
fn reference_run_conforms_and_measures_every_timing_clause() {
    let (result, acc) = sim_envelope(0x5EED);
    assert!(result.is_ok(), "reference run must conform: {result:?}");

    for clause in [
        CL_LIN_BREAK.id,
        CL_LIN_BREAK_DELIM.id,
        CL_LIN_FRAME_MAXLEN.id,
        CL_LIN_INTERBYTE_MAX.id,
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
    let timing = stablehal_lin::LinTiming::new(19_200);
    for seed in [1, 2, 3] {
        let (result, acc) = sim_envelope(seed);
        assert!(
            result.is_ok(),
            "seed {seed} unexpectedly failed: {result:?}"
        );
        let brk = acc.hull_for(CL_LIN_BREAK.id).unwrap();
        assert!(
            brk.min >= timing.break_min(),
            "seed {seed}: below the floor"
        );
    }
}
