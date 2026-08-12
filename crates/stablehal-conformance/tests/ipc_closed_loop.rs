//! Closed loop for the IPC conformance suite: the standard suite must pass
//! end to end, and its declared clause coverage must be honest.

use stablehal_conformance::ipc::runner::{run_standard_suite, run_suite};
use stablehal_conformance::ipc::{Case, STANDARD_SUITE};
use stablehal_ipc::IpcResponderBugs;

#[test]
fn standard_suite_passes() {
    let report = run_standard_suite();
    assert!(
        report.all_pass(),
        "suite failures:\n{}",
        report.failure_text()
    );
    assert_eq!(report.passed(), STANDARD_SUITE.len());
}

#[test]
fn suite_contains_both_positive_and_negative_cases() {
    // G3: the suite must actually prove the oracle catches things, not just
    // that conforming runs conform.
    let positives = STANDARD_SUITE
        .iter()
        .filter(|c| c.expect_breach.is_none())
        .count();
    let negatives = STANDARD_SUITE
        .iter()
        .filter(|c| c.expect_breach.is_some())
        .count();
    assert!(positives > 0, "no positive cases");
    assert!(negatives > 0, "no negative cases");
}

#[test]
fn declared_probes_are_real_clause_ids() {
    use stablehal_contract::Contract;
    use stablehal_ipc::{IpcConfig, IpcContract};

    let contract = IpcContract::new(IpcConfig::local_non_realtime());
    let known: Vec<&str> = contract.clauses().iter().map(|c| c.id).collect();
    for case in STANDARD_SUITE {
        for probe in case.probes {
            assert!(
                known.contains(probe),
                "case {} probes unknown clause {probe}",
                case.id
            );
        }
        if let Some(want) = case.expect_breach {
            assert!(
                known.contains(&want),
                "case {} expects unknown clause {want}",
                case.id
            );
        }
    }
}

#[test]
fn a_negative_case_that_stops_failing_is_reported_as_a_failure() {
    // Meta-test: if a case declares it expects a breach but the DUT is
    // actually conforming, the runner must fail it rather than pass it.
    // This is what stops the suite from rotting into a no-op.
    let bogus: &[Case] = &[Case {
        id: "IPC-WRONG-CORRELATION",
        description: "declares a breach but plants no bug",
        probes: &["IPC-CORRELATION"],
        bugs: IpcResponderBugs::default(),
        expect_breach: Some("IPC-CORRELATION"),
        background_load: 0,
    }];
    let report = run_suite(bogus);
    assert!(
        !report.all_pass(),
        "runner passed a negative case whose bug was absent"
    );
    assert!(report.failure_text().contains("but the run conformed"));
}
