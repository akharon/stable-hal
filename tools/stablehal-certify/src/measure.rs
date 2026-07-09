//! `stablehal-certify measure capture1.shc1 [capture2.shc1 ...] --timeout-ms N --json out.json`
//!
//! Parses each SHC1 capture, judges it against `RtuContract` while folding
//! every timing measurement into an envelope hull (min/max per clause,
//! across all input captures), and emits `stablehal-envelope-v1` JSON. A
//! VIOLATED capture aborts the whole run — an envelope built from a
//! non-conforming capture would misrepresent what "hardware conforms"
//! actually covers.

use std::fs;
use std::process::ExitCode;

use stablehal_contract::oracle::check_trace_measured;
use stablehal_contract::{EnvelopeAccumulator, Nanos, Verdict};
use stablehal_modbus::contract::RtuContract;
use stablehal_modbus::timing::RtuTiming;
use stablehal_shc1::parse_stream;

use stablehal_certify::arg_value;
use stablehal_certify::envelope::Envelope;

const CAPACITY: usize = 8;

pub fn run(args: &[String]) -> ExitCode {
    let paths: Vec<&String> = args.iter().take_while(|a| !a.starts_with("--")).collect();
    if paths.is_empty() {
        eprintln!(
            "usage: stablehal-certify measure <capture.shc1>... --timeout-ms <ms> --json <envelope.json>"
        );
        return ExitCode::FAILURE;
    }
    let timeout_ms: u64 = arg_value(args, "--timeout-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let json_out = arg_value(args, "--json");

    let mut acc: EnvelopeAccumulator<CAPACITY> = EnvelopeAccumulator::new();
    let mut traces = 0u64;

    for path in &paths {
        let bytes = match fs::read(path.as_str()) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("failed to read {path}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let (header, events, _report) = match parse_stream(&bytes) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("failed to parse SHC1 stream {path}: {e:?}");
                return ExitCode::FAILURE;
            }
        };
        let timing = RtuTiming::from_uart(&header.uart_cfg, Nanos::from_millis(timeout_ms));
        let contract = RtuContract::new(timing);
        let (verdict, run_acc) = check_trace_measured::<_, CAPACITY>(contract, &events);
        if let Verdict::Violated(v) = &verdict {
            eprintln!("capture {path} is not conforming, refusing to build an envelope from it:");
            eprintln!("{v}");
            return ExitCode::FAILURE;
        }
        acc.merge(&run_acc);
        traces += 1;
    }

    let envelope = Envelope::from_accumulator("modbus-rtu", "capture", traces, &acc);
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
