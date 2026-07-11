//! SPI conformance closed loop (risk T8): the reference register-pointer
//! device must pass every standard case, and the suite's declared `probes`
//! must cover every clause `SpiContract` actually checks in `step` —
//! `SPI-MODE-FAULT` (unreachable from a conforming reference master/device
//! in this world's topology) and `SPI-DATA-VALID` (vestigial: declared but
//! never checked in `step`, see `stablehal_spi`'s crate-level doc comment)
//! are excluded, same honesty pattern as MODBUS's `RTU-CHAR-OVERLAP`.

use stablehal_conformance::spi::runner::run_standard_suite;
use stablehal_conformance::spi::STANDARD_SUITE;

#[test]
fn reference_device_passes_standard_suite() {
    let report = run_standard_suite();
    assert!(
        report.all_pass(),
        "reference DUT failed some cases:\n{}",
        report.failure_text()
    );
    assert_eq!(report.passed(), STANDARD_SUITE.len());
}

#[test]
fn every_checked_clause_is_probed_by_the_suite() {
    let report = run_standard_suite();
    let probed = report.probed_clauses(STANDARD_SUITE);
    let must_be_probed = [
        "SPI-TRANSACTION",
        "SPI-CS-TIMING",
        "SPI-MODE-DISC",
        "SPI-XFER-TIME",
        "SPI-CS-MIN-IDLE",
    ];
    for clause in must_be_probed {
        assert!(
            probed.contains(&clause),
            "clause {clause} is not probed by any STANDARD_SUITE case"
        );
    }
}
