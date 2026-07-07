//! WP1 / E5: run STANDARD_SUITE against the rmodbus adapter.
//! Do NOT assert all_pass. Assert the *report is stable* and print it.
//! First run establishes the observed outcome; then pin it as a golden value
//! so regressions in the adapter are caught.

use stablehal_conformance::runner::{run_suite, SuiteConfig};
use stablehal_conformance::{pattern, STANDARD_SUITE};
use stablehal_contract::Nanos;
use stablehal_modbus::RtuTiming;
use stablehal_sim::Actor;
use stablehal_uart::UartConfig;

use rmodbus_slave::RmodbusSlaveActor;

fn make_dut() -> Box<dyn Actor> {
    let timing = RtuTiming::from_uart(&UartConfig::rtu_default(9600), Nanos::from_millis(50));
    Box::new(RmodbusSlaveActor::new(
        0x11,
        timing,
        Nanos::from_millis(1),
        pattern,
    ))
}

#[test]
fn rmodbus_versus_standard_suite() {
    let cfg = SuiteConfig::default_9600();
    let report = run_suite(&cfg, STANDARD_SUITE, make_dut);

    // Print the per-case table.
    println!("Rmodbus adapter vs STANDARD_SUITE");
    println!("rmodbus version: 0.12.2");
    println!();
    println!("{:<28} {:>7}  {}", "case", "pass", "detail");
    println!("{}", "-".repeat(90));
    for outcome in &report.outcomes {
        let status = if outcome.pass { "PASS" } else { "FAIL" };
        println!("{:<28} {:>7}  {}", outcome.id, status, outcome.detail);
    }
    println!("{}", "-".repeat(90));
    println!("passed: {}/{}", report.passed(), STANDARD_SUITE.len());
    println!();

    // Golden expectation: pin the observed outcome so regressions are caught.
    // As of rmodbus 0.12.2, the observed pass count is recorded here.
    // When the pinned version changes, update this value.
    let observed = report.passed();
    assert!(
        observed >= 8,
        "rmodbus adapter passed only {} cases — unexpected regression. \
         Full report:\n{}",
        observed,
        report
            .outcomes
            .iter()
            .map(|o| format!("{}: {} — {}", o.id, if o.pass { "PASS" } else { "FAIL" }, o.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Pin the exact count to catch any regressions.
    // On first run with a new rmodbus version, update this line.
    assert_eq!(
        observed, 10,
        "rmodbus adapter passed {} cases; expected 10. \
         If rmodbus version changed, update the golden value. \
         Failures (expected): READ-BAD-ADDR (register space larger than \
         reference), READ-BAD-COUNT (count=0 handled differently), \
         WRITE-SINGLE-BAD-ADDR (no 128-reg cap).",
        observed
    );
}
