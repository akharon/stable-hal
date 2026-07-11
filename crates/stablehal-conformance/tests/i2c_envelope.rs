//! Envelope containment for I2C (risk T11): `sim_envelope` must judge
//! conforming (the reference master/device never breach their own contract)
//! and must report a nonempty hull for every timing clause `I2cContract`
//! measures through `step_measured`.

use stablehal_conformance::i2c::runner::sim_envelope;
use stablehal_i2c::contract::{CL_I2C_BUS_FREE, CL_I2C_STRETCH_MAX};

#[test]
fn reference_run_conforms_and_measures_every_timing_clause() {
    let (result, acc) = sim_envelope(0x5EED);
    assert!(result.is_ok(), "reference run must conform: {result:?}");

    for clause in [CL_I2C_BUS_FREE.id, CL_I2C_STRETCH_MAX.id] {
        let hull = acc.hull_for(clause).unwrap_or_else(|| {
            panic!("expected clause {clause} to have been measured at least once")
        });
        assert!(hull.samples > 0);
        assert!(hull.min <= hull.max);
    }
}

#[test]
fn distinct_seeds_yield_hulls_within_the_configured_envelope() {
    for seed in [1, 2, 3] {
        let (result, acc) = sim_envelope(seed);
        assert!(
            result.is_ok(),
            "seed {seed} unexpectedly failed: {result:?}"
        );
        let bus_free = acc.hull_for(CL_I2C_BUS_FREE.id).unwrap();
        assert!(
            bus_free.min >= stablehal_i2c::I2cTiming::standard().bus_free_min,
            "seed {seed}: below the floor"
        );
    }
}
