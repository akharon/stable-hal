//! `stablehal-certify contain --hw hw.json --sim sim.json [--json cert.json]`
//!
//! Compares a hardware-measured envelope against a sim-explored envelope,
//! clause by clause, and emits a `stablehal-certificate-v2`. Exit code is
//! SUCCESS iff every measured clause is COVERED — see
//! docs/user-guide-containment.md for what that claim means.

use std::fs;
use std::process::ExitCode;

use stablehal_certify::arg_value;
use stablehal_certify::contain::{certificate_json, contain};
use stablehal_certify::envelope::Envelope;

pub fn run(args: &[String]) -> ExitCode {
    let (Some(hw_path), Some(sim_path)) = (arg_value(args, "--hw"), arg_value(args, "--sim"))
    else {
        eprintln!(
            "usage: stablehal-certify contain --hw <hw.json> --sim <sim.json> [--json <cert.json>]"
        );
        return ExitCode::FAILURE;
    };
    let json_out = arg_value(args, "--json");

    let hw = match Envelope::read(&hw_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to read hw envelope {hw_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sim = match Envelope::read(&sim_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to read sim envelope {sim_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let report = contain(&hw, &sim);
    let certificate = certificate_json(&hw, &sim, &report);
    print!("{certificate}");
    if let Some(out_path) = json_out {
        if let Err(e) = fs::write(&out_path, &certificate) {
            eprintln!("failed to write {out_path}: {e}");
            return ExitCode::FAILURE;
        }
    }

    if report.all_covered() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
