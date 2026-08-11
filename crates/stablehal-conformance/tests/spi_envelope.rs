//! Envelope containment for SPI (risk T12): `sim_envelope` must judge
//! conforming (the reference master/device never breach their own contract)
//! and must report a nonempty hull for every timing clause `SpiContract`
//! measures through `step_measured` — `SPI-CS-MIN-IDLE` and `SPI-XFER-TIME`.

use stablehal_conformance::spi::runner::sim_envelope;
use stablehal_spi::contract::{CL_SPI_CS_MIN_IDLE, CL_SPI_XFER_TIME};

#[test]
fn reference_run_conforms_and_measures_every_timing_clause() {
    let (result, acc) = sim_envelope(0x5EED);
    assert!(result.is_ok(), "reference run must conform: {result:?}");

    for clause in [CL_SPI_CS_MIN_IDLE.id, CL_SPI_XFER_TIME.id] {
        let hull = acc.hull_for(clause).unwrap_or_else(|| {
            panic!("expected clause {clause} to have been measured at least once")
        });
        assert!(hull.samples > 0);
        assert!(hull.min <= hull.max);
    }
}

#[test]
fn distinct_seeds_conform_and_stay_within_the_xfer_envelope() {
    let config = stablehal_spi::SpiConfig::default_1mhz();
    // The per-byte transfer envelope is 8× the SCLK period ± tolerance; every
    // measured SPI-XFER-TIME sample must sit at or above the envelope floor.
    let floor = config
        .sclk_period
        .mul_ratio(8, 1)
        .saturating_sub(config.sclk_tolerance);
    for seed in [1, 2, 3] {
        let (result, acc) = sim_envelope(seed);
        assert!(
            result.is_ok(),
            "seed {seed} unexpectedly failed: {result:?}"
        );
        let xfer = acc.hull_for(CL_SPI_XFER_TIME.id).unwrap();
        assert!(
            xfer.min >= floor,
            "seed {seed}: xfer time {} below envelope floor {}",
            xfer.min,
            floor
        );
    }
}
