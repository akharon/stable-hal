//! Conformance demo: run the contract-derived suite against the reference
//! slave (must pass 100%) and against a catalog of deliberately
//! nonconforming slaves (each must fail, with the breached clause named).
//!
//! This is the same suite that later runs on-target against real hardware —
//! the `no_std` case definitions and oracle are identical.

use stablehal_conformance::runner::{run_suite, SlaveActor, SuiteConfig};
use stablehal_conformance::{pattern, DUT_UNIT, STANDARD_SUITE};
use stablehal_contract::Nanos;
use stablehal_modbus::{RegisterBank, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_sim::Actor;
use stablehal_uart::UartConfig;

fn timing() -> RtuTiming {
    RtuTiming::from_uart(&UartConfig::rtu_default(9600), Nanos::from_millis(50))
}

fn dut(bugs: SlaveBugs) -> impl FnMut() -> Box<dyn Actor> {
    move || {
        Box::new(SlaveActor::new(
            SlaveConfig {
                unit: DUT_UNIT,
                timing: timing(),
                response_delay: Nanos::from_millis(1),
                bugs,
            },
            RegisterBank::with_pattern(pattern),
        ))
    }
}

/// Extract the clause id ("RTU-…") from a violation detail, if present.
fn clause_of(text: &str) -> Option<&str> {
    let start = text.find("clause ")? + "clause ".len();
    let rest = &text[start..];
    let end = rest.find(' ')?;
    Some(&rest[..end])
}

fn main() {
    let cfg = SuiteConfig::default_9600();
    let catalog: Vec<(&str, SlaveBugs)> = vec![
        ("reference (conforming)", SlaveBugs::default()),
        (
            "responds inside t3.5",
            SlaveBugs {
                respond_inside_t35: true,
                ..Default::default()
            },
        ),
        (
            "corrupt CRC",
            SlaveBugs {
                corrupt_crc: true,
                ..Default::default()
            },
        ),
        (
            "gap > t1.5 in response",
            SlaveBugs {
                intra_frame_gap: Some(timing().t15 + timing().t15),
                ..Default::default()
            },
        ),
        (
            "response after timeout",
            SlaveBugs {
                late_by: Some(Nanos::from_millis(10)),
                ..Default::default()
            },
        ),
        (
            "wrong function code",
            SlaveBugs {
                wrong_fc: true,
                ..Default::default()
            },
        ),
        (
            "wrong unit address",
            SlaveBugs {
                wrong_unit: true,
                ..Default::default()
            },
        ),
    ];

    println!(
        "StableHAL conformance suite — {} cases, MODBUS RTU slave DUT (unit 0x{:02X}, 9600-8E1)\n",
        STANDARD_SUITE.len(),
        DUT_UNIT
    );
    println!(
        "{:<28} {:>7}  first breached clause / detail",
        "device under test", "passed"
    );
    println!("{}", "-".repeat(100));

    let mut ok = true;
    for (i, (name, bugs)) in catalog.iter().enumerate() {
        let report = run_suite(&cfg, STANDARD_SUITE, dut(*bugs));
        let failed = report.failed();
        let detail = failed
            .first()
            .map(|o| {
                let d = clause_of(&o.detail).unwrap_or(&o.detail);
                format!("{} (case {})", d, o.id)
            })
            .unwrap_or_else(|| "—".to_string());
        println!(
            "{:<28} {:>3}/{:<3}  {}",
            name,
            report.passed(),
            STANDARD_SUITE.len(),
            detail
        );
        let is_reference = i == 0;
        if is_reference && !report.all_pass() {
            eprintln!(
                "  ✗ reference slave must pass everything:\n{}",
                report.failure_text()
            );
            ok = false;
        }
        if !is_reference && report.all_pass() {
            eprintln!("  ✗ nonconforming device escaped the suite: {name}");
            ok = false;
        }
    }

    println!("{}", "-".repeat(100));
    if ok {
        println!("closed loop holds: conformance ⇔ contract, in both directions ✓");
    } else {
        println!("closed loop BROKEN ✗");
        std::process::exit(1);
    }
}
