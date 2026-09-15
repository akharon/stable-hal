//! `stablehal-certify sim-envelope --seeds 50 [--protocol modbus-rtu|ipc] [--json sim.json]`
//!
//! Runs the reference driver against the reference DUT over `--seeds`
//! consecutive seeds (0..seeds), folding every timing measurement — passing
//! or not — into an envelope hull. This is the "what did the simulator
//! actually exercise" side of envelope containment. A seed whose reference
//! run does not conform aborts the run: the reference is supposed to be
//! conforming by construction, so a breach here means the DST harness
//! itself found a bug, not that the envelope is incomplete.
//!
//! `--protocol` defaults to `modbus-rtu`; `ipc` folds
//! `stablehal_conformance::ipc::runner::sim_envelope_sweep` instead.

use std::fs;
use std::process::ExitCode;

use stablehal_contract::EnvelopeAccumulator;

use stablehal_certify::arg_value;
use stablehal_certify::envelope::Envelope;

const CAPACITY: usize = 8;

fn run_rtu(seeds: u64) -> Result<Envelope, String> {
    let mut cfg = stablehal_conformance::runner::SuiteConfig::default_9600();
    let mut acc: EnvelopeAccumulator<CAPACITY> = EnvelopeAccumulator::new();

    for seed in 0..seeds {
        cfg.seed = seed;
        let (result, run_acc) = stablehal_conformance::runner::sim_envelope(&cfg);
        if let Err(e) = result {
            return Err(format!("seed {seed}: reference run did not conform: {e}"));
        }
        acc.merge(&run_acc);
    }
    Ok(Envelope::from_accumulator("modbus-rtu", "sim", seeds, &acc))
}

fn run_ipc(seeds: u64) -> Result<Envelope, String> {
    let (result, acc) = stablehal_conformance::ipc::runner::sim_envelope_sweep(seeds);
    result.map_err(|e| format!("sim-envelope-sweep did not conform: {e}"))?;
    Ok(Envelope::from_accumulator("ipc", "sim", seeds, &acc))
}

pub fn run(args: &[String]) -> ExitCode {
    let seeds: u64 = arg_value(args, "--seeds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let json_out = arg_value(args, "--json");
    let protocol = arg_value(args, "--protocol").unwrap_or_else(|| "modbus-rtu".to_string());

    let result = match protocol.as_str() {
        "modbus-rtu" => run_rtu(seeds),
        "ipc" => run_ipc(seeds),
        other => Err(format!(
            "unrecognized --protocol {other} (expected modbus-rtu or ipc)"
        )),
    };

    let envelope = match result {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let json = envelope.to_json();
    print!("{json}");
    if let Some(out_path) = json_out {
        if let Err(e) = fs::write(&out_path, &json) {
            eprintln!("failed to write {out_path}: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
