//! `stablehal-certify sim-envelope --seeds 50 [--json sim.json]`
//!
//! Runs the reference master against the reference slave over `--seeds`
//! consecutive seeds (0..seeds), folding every timing measurement — passing
//! or not — into an envelope hull. This is the "what did the simulator
//! actually exercise" side of envelope containment. A seed whose reference
//! run does not conform aborts the run: the reference is supposed to be
//! conforming by construction, so a breach here means the DST harness
//! itself found a bug, not that the envelope is incomplete.

use std::fs;
use std::process::ExitCode;

use stablehal_contract::EnvelopeAccumulator;

use stablehal_certify::arg_value;
use stablehal_certify::envelope::Envelope;

const CAPACITY: usize = 8;

pub fn run(args: &[String]) -> ExitCode {
    let seeds: u64 = arg_value(args, "--seeds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let json_out = arg_value(args, "--json");

    let mut cfg = stablehal_conformance::runner::SuiteConfig::default_9600();
    let mut acc: EnvelopeAccumulator<CAPACITY> = EnvelopeAccumulator::new();

    for seed in 0..seeds {
        cfg.seed = seed;
        let (result, run_acc) = stablehal_conformance::runner::sim_envelope(&cfg);
        if let Err(e) = result {
            eprintln!("seed {seed}: reference run did not conform: {e}");
            return ExitCode::FAILURE;
        }
        acc.merge(&run_acc);
    }

    let envelope = Envelope::from_accumulator("modbus-rtu", "sim", seeds, &acc);
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
