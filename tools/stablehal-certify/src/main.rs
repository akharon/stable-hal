//! `stablehal-certify` — host tool for MODBUS RTU envelope containment
//! certification. Subcommands:
//!
//!   judge <capture.shc1> ...           judge a capture against the RTU
//!                                       contract oracle (WP3 §3.2)
//!   measure <capture.shc1>... ...      fold conforming captures into a
//!                                       hardware-measured envelope
//!   sim-envelope --seeds N ...         fold N reference-driver DST runs
//!                                       into a sim-explored envelope
//!   contain --hw H --sim S ...         check hw envelope ⊆ sim envelope,
//!                                       per clause; emit a v2 certificate
//!
//! No subcommand is an alias for `judge`, for backward compatibility.
//! Hand-rolled JSON throughout — zero third-party deps (G1).

mod cli_contain;
mod cli_sim_envelope;
mod judge;
mod measure;

use std::env;
use std::process::ExitCode;

fn usage() {
    eprintln!(
        "usage: stablehal-certify <judge|measure|sim-envelope|contain> [args]\n\
         \n\
         stablehal-certify judge <capture.shc1> --timeout-ms <ms> [--json <report.json>]\n\
         stablehal-certify measure <capture.shc1>... --timeout-ms <ms> --json <envelope.json>\n\
         stablehal-certify sim-envelope --seeds <n> [--json <envelope.json>]\n\
         stablehal-certify contain --hw <hw.json> --sim <sim.json> [--json <cert.json>]"
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(first) = args.first() else {
        usage();
        return ExitCode::FAILURE;
    };

    match first.as_str() {
        "judge" => judge::run(&args[1..]),
        "measure" => measure::run(&args[1..]),
        "sim-envelope" => cli_sim_envelope::run(&args[1..]),
        "contain" => cli_contain::run(&args[1..]),
        // No recognized subcommand and the first arg doesn't look like a
        // flag: treat it as `judge <path>` for backward compatibility with
        // the pre-subcommand invocation.
        _ if !first.starts_with('-') => judge::run(&args),
        _ => {
            usage();
            ExitCode::FAILURE
        }
    }
}
