//! The closed loop, proven in simulation:
//! * the reference slave (conforming by construction) passes the generated
//!   conformance suite 100%,
//! * every cataloged contract violation is caught and attributed to the
//!   exact clause,
//! * the real master driver survives a DST sweep across the whole
//!   environment envelope with zero wire violations and zero wrong data,
//! * a planted master-side bug is caught by the same oracle,
//! * seed replay is exact (trace-hash equality).

use std::cell::RefCell;
use std::rc::Rc;

use stablehal_conformance::runner::{
    rtu_observer, run_suite, OwnedRequest, ScriptOutcome, ScriptedMaster, SlaveActor, SuiteConfig,
    Txn, Verify,
};
use stablehal_conformance::{pattern, DUT_UNIT, STANDARD_SUITE};
use stablehal_contract::{Nanos, SplitMix64};
use stablehal_modbus::{MasterConfig, RegisterBank, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_sim::{Actor, Sim, SimConfig};
use stablehal_uart::UartConfig;

const UART: UartConfig = UartConfig::rtu_default(9600);

fn timing() -> RtuTiming {
    RtuTiming::from_uart(&UART, Nanos::from_millis(50))
}

fn dut_factory(bugs: SlaveBugs) -> impl FnMut() -> Box<dyn Actor> {
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

// ---------------------------------------------------------------------------
// M3: conformance closed loop
// ---------------------------------------------------------------------------

#[test]
fn reference_slave_passes_standard_suite() {
    let report = run_suite(
        &SuiteConfig::default_9600(),
        STANDARD_SUITE,
        dut_factory(SlaveBugs::default()),
    );
    assert!(
        report.all_pass(),
        "reference slave failed:\n{}",
        report.failure_text()
    );
    assert_eq!(report.passed(), STANDARD_SUITE.len());
}

#[test]
fn every_cataloged_bug_is_caught_with_the_right_clause() {
    let catalog: &[(&str, SlaveBugs, &str)] = &[
        (
            "responds before t3.5 frame silence",
            SlaveBugs {
                respond_inside_t35: true,
                ..Default::default()
            },
            "RTU-T35-FRAME",
        ),
        (
            "emits corrupt CRC",
            SlaveBugs {
                corrupt_crc: true,
                ..Default::default()
            },
            "RTU-CRC",
        ),
        (
            "gaps > t1.5 inside response",
            SlaveBugs {
                intra_frame_gap: Some(timing().t15 + timing().t15),
                ..Default::default()
            },
            "RTU-T15-INTRA",
        ),
        (
            "responds after the timeout",
            SlaveBugs {
                late_by: Some(Nanos::from_millis(10)),
                ..Default::default()
            },
            "RTU-RESP-TIME",
        ),
        (
            "answers with an alien function code",
            SlaveBugs {
                wrong_fc: true,
                ..Default::default()
            },
            "RTU-RESP-FC",
        ),
        (
            "answers with the wrong unit address",
            SlaveBugs {
                wrong_unit: true,
                ..Default::default()
            },
            "RTU-RESP-UNIT",
        ),
    ];

    for (name, bugs, expected_clause) in catalog {
        let report = run_suite(
            &SuiteConfig::default_9600(),
            STANDARD_SUITE,
            dut_factory(*bugs),
        );
        assert!(
            !report.all_pass(),
            "bug '{}' escaped the conformance suite entirely",
            name
        );
        let text = report.failure_text();
        assert!(
            text.contains(expected_clause),
            "bug '{}' was caught but not attributed to {}:\n{}",
            name,
            expected_clause,
            text
        );
    }
}

// ---------------------------------------------------------------------------
// M2: DST sweep of the real master driver
// ---------------------------------------------------------------------------

fn build_script(seed: u64) -> Vec<Txn> {
    let mut r = SplitMix64::new(seed ^ 0xABCD_EF01);
    let mut txns = Vec::new();
    for block in 0..3u16 {
        // Read from the never-written region: must match the pattern.
        txns.push(Txn {
            unit: DUT_UNIT,
            request: OwnedRequest::ReadHolding {
                addr: 64 + block * 8,
                count: 8,
            },
            verify: Verify::Pattern {
                addr: 64 + block * 8,
            },
            needs_prev_ok: false,
        });
        // Write + readback.
        let addr = (r.range_inclusive(0, 40)) as u16;
        let value = r.next_u64() as u16;
        txns.push(Txn {
            unit: DUT_UNIT,
            request: OwnedRequest::WriteSingle { addr, value },
            verify: Verify::None,
            needs_prev_ok: false,
        });
        txns.push(Txn {
            unit: DUT_UNIT,
            request: OwnedRequest::ReadHolding { addr, count: 1 },
            verify: Verify::Values(vec![value]),
            needs_prev_ok: true,
        });
        // Multi-write + readback.
        let maddr = 48 + (r.range_inclusive(0, 8)) as u16;
        let values: Vec<u16> = (0..4).map(|_| r.next_u64() as u16).collect();
        txns.push(Txn {
            unit: DUT_UNIT,
            request: OwnedRequest::WriteMultiple {
                addr: maddr,
                values: values.clone(),
            },
            verify: Verify::None,
            needs_prev_ok: false,
        });
        txns.push(Txn {
            unit: DUT_UNIT,
            request: OwnedRequest::ReadHolding {
                addr: maddr,
                count: 4,
            },
            verify: Verify::Values(values),
            needs_prev_ok: true,
        });
    }
    txns
}

struct DstRun {
    outcome: Rc<RefCell<ScriptOutcome>>,
    result: Result<(), String>,
    hash: u64,
}

fn dst_run(seed: u64, corrupt_ppm: u32, skip_pre_tx_silence: bool) -> DstRun {
    let timing = timing();
    let mut mcfg = MasterConfig::new(timing);
    mcfg.skip_pre_tx_silence = skip_pre_tx_silence;
    let (master, outcome) = ScriptedMaster::new(mcfg, build_script(seed), pattern);

    let mut env = SplitMix64::new(seed ^ 0x5EED_0051);
    let response_delay = Nanos::from_nanos(env.range_inclusive(
        0,
        (timing.response_timeout.as_nanos() / 2).saturating_sub(timing.t35.as_nanos()),
    ));

    let slave = SlaveActor::new(
        SlaveConfig {
            unit: DUT_UNIT,
            timing,
            response_delay,
            bugs: SlaveBugs::default(),
        },
        RegisterBank::with_pattern(pattern),
    );

    let mut cfg = SimConfig::new(seed, UART);
    cfg.corrupt_ppm = corrupt_ppm;
    let mut sim = Sim::new(cfg, Box::new(master), Box::new(slave));
    sim.add_observer(rtu_observer(timing, true, true));

    let result = sim
        .run_until(Nanos::from_secs(60))
        .map_err(|e| e.to_string())
        .and_then(|s| {
            if s.done[0] {
                Ok(())
            } else {
                Err("master script did not finish".into())
            }
        });
    DstRun {
        outcome,
        result,
        hash: sim.trace_hash(),
    }
}

#[test]
fn master_driver_survives_the_envelope_sweep() {
    let mut gave_up_total = 0usize;
    for seed in 0..100 {
        let run = dst_run(seed, 2_000, false);
        assert!(
            run.result.is_ok(),
            "seed {}: wire violation or hang: {:?}",
            seed,
            run.result
        );
        let out = run.outcome.borrow();
        assert!(
            out.property_failures.is_empty(),
            "seed {}: silent data corruption: {:?}",
            seed,
            out.property_failures
        );
        gave_up_total += out.gave_up;
    }
    // With 0.2% per-character corruption some transactions may honestly give
    // up — that is contract-legal. What must never happen is wrong data.
    let _ = gave_up_total;
}

#[test]
fn planted_master_bug_is_caught_by_the_same_oracle() {
    let run = dst_run(7, 0, true);
    let err = run
        .result
        .expect_err("buggy master escaped the wire oracle");
    assert!(
        err.contains("RTU-T35-FRAME") || err.contains("RTU-MASTER-WAIT"),
        "unexpected clause: {}",
        err
    );
}

#[test]
fn seed_replay_is_exact_and_seeds_differ() {
    let a = dst_run(11, 2_000, false);
    let b = dst_run(11, 2_000, false);
    assert_eq!(a.hash, b.hash, "same seed must replay byte-for-byte");
    let c = dst_run(12, 2_000, false);
    assert_ne!(a.hash, c.hash, "different seeds should explore differently");
}
