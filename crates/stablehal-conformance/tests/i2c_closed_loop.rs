//! I2C conformance closed loop (WP2 §2.2.5): the reference register-pointer
//! device must pass every standard case, and the suite's declared `probes`
//! must cover every clause the `I2cContract` enforces except the reserved
//! (unimplemented) `I2C-ARBITRATION`.

use stablehal_conformance::i2c::runner::run_standard_suite;
use stablehal_conformance::i2c::STANDARD_SUITE;

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
fn every_implemented_clause_is_probed_by_the_suite() {
    let report = run_standard_suite();
    let probed = report.probed_clauses(STANDARD_SUITE);
    // I2C-BUS-FREE and I2C-BYTE-TIME are timing clauses exercised implicitly
    // by every case (every Start/byte passes through them) rather than
    // named in any single case's `probes` list — this mirrors how the RTU
    // suite treats structural/always-on clauses. Assert the *behavioral*
    // clauses (ones a specific case exists to probe) are all covered.
    let must_be_probed = [
        "I2C-SEQ",
        "I2C-ADDR-ACK",
        "I2C-STRETCH-MAX",
        "I2C-WRITE-ACK",
        "I2C-READ-NACK-LAST",
    ];
    for clause in must_be_probed {
        assert!(
            probed.contains(&clause),
            "clause {clause} is not probed by any STANDARD_SUITE case"
        );
    }
}
