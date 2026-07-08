//! `stablehal-certify capture.shc1 --timeout-ms 50 [--json report.json]`
//!
//! Parses an SHC1 capture (WP3 §3.1), maps records to `RtuEvent`s exactly as
//! the sim-side `rtu_observer` does, judges the trace with the same
//! `RtuContract` oracle used in simulation, and emits a conformance
//! certificate (WP3 §3.2). Hand-rolled JSON — zero third-party deps (G1).

use std::env;
use std::fs;
use std::process::ExitCode;

use stablehal_contract::oracle::check_trace;
use stablehal_contract::{Nanos, Verdict};
use stablehal_modbus::contract::RtuContract;
use stablehal_modbus::timing::RtuTiming;
use stablehal_shc1::{parse_stream, Header};

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn framing_label(header: &Header) -> String {
    let cfg = header.uart_cfg;
    let parity = match cfg.parity {
        stablehal_uart::Parity::None => 'N',
        stablehal_uart::Parity::Even => 'E',
        stablehal_uart::Parity::Odd => 'O',
    };
    format!("{}{}{}", cfg.data_bits, parity, cfg.stop_bits)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!(
            "usage: stablehal-certify <capture.shc1> --timeout-ms <ms> [--json <report.json>]"
        );
        return ExitCode::FAILURE;
    };
    let timeout_ms: u64 = arg_value(&args, "--timeout-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let json_out = arg_value(&args, "--json");

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let (header, events, report) = match parse_stream(&bytes) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("failed to parse SHC1 stream: {e:?}");
            return ExitCode::FAILURE;
        }
    };

    let timing = RtuTiming::from_uart(&header.uart_cfg, Nanos::from_millis(timeout_ms));
    let contract = RtuContract::new(timing);
    let verdict = check_trace(contract, &events);

    let trace_hash = stablehal_contract::trace::trace_hash(events.iter());
    let (verdict_str, violation_json) = match &verdict {
        Verdict::Conforming { .. } => ("CONFORMING".to_string(), "null".to_string()),
        Verdict::Violated(v) => (
            "VIOLATED".to_string(),
            format!(
                "{{\"clause\":\"{}\",\"citation\":\"{}\",\"at_ns\":{},\"step\":{},\"message\":\"{}\"}}",
                json_escape(v.breach.clause.id),
                json_escape(v.breach.clause.citation),
                v.at.as_nanos(),
                v.step,
                json_escape(&v.to_string()),
            ),
        ),
    };

    let framing = framing_label(&header);
    let certificate = format!(
        "{{\n  \"format\": \"stablehal-certificate-v1\",\n  \"contract\": {{ \"id\": \"modbus-rtu\", \"version\": \"0.1\", \"clauses\": 12 }},\n  \"uart\": {{ \"baud\": {}, \"framing\": \"{}\" }},\n  \"timing\": {{ \"t15_ns\": {}, \"t35_ns\": {}, \"response_timeout_ns\": {} }},\n  \"capture\": {{ \"records\": {}, \"gaps\": {}, \"trace_hash\": \"{:016x}\" }},\n  \"verdict\": \"{}\",\n  \"violation\": {}\n}}\n",
        header.baud,
        framing,
        timing.t15.as_nanos(),
        timing.t35.as_nanos(),
        timing.response_timeout.as_nanos(),
        report.records,
        report.capture_gaps,
        trace_hash,
        verdict_str,
        violation_json,
    );

    print!("{certificate}");
    if let Some(out_path) = json_out {
        if let Err(e) = fs::write(&out_path, &certificate) {
            eprintln!("failed to write {out_path}: {e}");
            return ExitCode::FAILURE;
        }
    }

    match verdict {
        Verdict::Conforming { .. } => ExitCode::SUCCESS,
        Verdict::Violated(_) => ExitCode::FAILURE,
    }
}
