//! DST sweep demo: the real `no_std` MODBUS RTU master driver, exercised
//! across the whole environment envelope (latency jitter + character
//! corruption), judged online by the UART line contracts and the RTU wire
//! contract, with semantic property checks on every transaction.
//!
//! Usage:
//!   modbus-master [--seeds N] [--corrupt PPM] [--replay-check SEED]
//!
//! Exit code 0 = every seed conforming and no silent data corruption.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use stablehal_conformance::runner::{
    rtu_observer, OwnedRequest, ScriptOutcome, ScriptedMaster, SlaveActor, Txn, Verify,
};
use stablehal_conformance::{pattern, DUT_UNIT};
use stablehal_contract::{Nanos, SplitMix64};
use stablehal_modbus::{MasterConfig, RegisterBank, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_sim::{Sim, SimConfig};
use stablehal_uart::UartConfig;

const UART: UartConfig = UartConfig::rtu_default(9600);

fn timing() -> RtuTiming {
    RtuTiming::from_uart(&UART, Nanos::from_millis(50))
}

fn build_script(seed: u64) -> Vec<Txn> {
    let mut r = SplitMix64::new(seed ^ 0xABCD_EF01);
    let mut txns = Vec::new();
    for block in 0..3u16 {
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
        let addr = r.range_inclusive(0, 40) as u16;
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
        let maddr = 48 + r.range_inclusive(0, 8) as u16;
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

struct RunResult {
    violation: Option<String>,
    outcome: Rc<RefCell<ScriptOutcome>>,
    hash: u64,
    events: usize,
}

fn run_one(seed: u64, corrupt_ppm: u32) -> RunResult {
    let timing = timing();
    let (master, outcome) =
        ScriptedMaster::new(MasterConfig::new(timing), build_script(seed), pattern);

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

    let violation = match sim.run_until(Nanos::from_secs(60)) {
        Ok(s) if s.done[0] => None,
        Ok(_) => Some("master script did not finish".to_string()),
        Err(e) => Some(e.to_string()),
    };
    RunResult {
        violation,
        outcome,
        hash: sim.trace_hash(),
        events: sim.trace().len(),
    }
}

fn arg_value(args: &[String], key: &str) -> Option<u64> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seeds = arg_value(&args, "--seeds").unwrap_or(1000);
    let corrupt_ppm = arg_value(&args, "--corrupt").unwrap_or(2000) as u32;

    if let Some(seed) = arg_value(&args, "--replay-check") {
        let a = run_one(seed, corrupt_ppm);
        let b = run_one(seed, corrupt_ppm);
        println!("seed {seed}: hash {:016x} vs {:016x}", a.hash, b.hash);
        if a.hash == b.hash {
            println!("replay EXACT ✓");
            std::process::exit(0);
        }
        println!("replay DIVERGED ✗");
        std::process::exit(1);
    }

    println!("StableHAL DST sweep — MODBUS RTU master over contract-simulated 9600-8E1 wire");
    println!(
        "seeds: {seeds}, corruption: {corrupt_ppm} ppm/char, latency envelope: {}",
        SimConfig::new(0, UART).latency
    );

    let mut hashes = BTreeSet::new();
    let mut violations = 0usize;
    let mut property_failures = 0usize;
    let mut gave_up = 0usize;
    let mut completed = 0usize;
    let mut events = 0usize;

    for seed in 0..seeds {
        let r = run_one(seed, corrupt_ppm);
        hashes.insert(r.hash);
        events += r.events;
        if let Some(v) = &r.violation {
            violations += 1;
            eprintln!("seed {seed}: VIOLATION: {v}");
        }
        let out = r.outcome.borrow();
        if !out.property_failures.is_empty() {
            property_failures += out.property_failures.len();
            eprintln!("seed {seed}: WRONG DATA: {:?}", out.property_failures);
        }
        gave_up += out.gave_up;
        completed += out.completed;
    }

    println!("---");
    println!("distinct wire schedules explored : {}", hashes.len());
    println!("total wire events judged         : {events}");
    println!("transactions completed           : {completed}");
    println!("transactions honestly given up   : {gave_up} (contract-legal under fault injection)");
    println!("wire contract violations         : {violations}");
    println!("silent data corruptions          : {property_failures}");
    if violations == 0 && property_failures == 0 {
        println!("VERDICT: conforming across the entire explored envelope ✓");
    } else {
        println!("VERDICT: NONCONFORMING ✗");
        std::process::exit(1);
    }
}
