//! CAN conformance closed loop (risk T8): the reference responder must pass
//! every standard case, and the suite's declared `probes` must cover every
//! clause `CanContract` actually checks from a `CanEvent::Frame`
//! observation. See `stablehal_conformance::can`'s module doc comment for
//! why the other 6 clauses are excluded (never checked in `step`, or need
//! bus-monitor-level events this reference world never emits).

use stablehal_conformance::can::runner::run_standard_suite;
use stablehal_conformance::can::STANDARD_SUITE;

#[test]
fn reference_responder_passes_standard_suite() {
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
    let must_be_probed = ["CAN-ID-FIELD", "CAN-RTR-DISC", "CAN-ACK"];
    for clause in must_be_probed {
        assert!(
            probed.contains(&clause),
            "clause {clause} is not probed by any STANDARD_SUITE case"
        );
    }
}
