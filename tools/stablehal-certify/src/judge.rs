//! `stablehal-certify judge capture.shc1 --timeout-ms 50 [--json report.json]`
//! (also the default when no subcommand is given, for backward compatibility)
//!
//! Parses an SHC1 capture, dispatches on its `contract` header field (WP3
//! §3.1) to either the RTU or the IPC oracle, and emits a conformance
//! certificate (WP3 §3.2). Hand-rolled JSON — zero third-party deps (G1).

use std::fs;
use std::process::ExitCode;

use stablehal_contract::oracle::check_trace;
use stablehal_contract::{Nanos, Verdict};
use stablehal_ipc::contract::{IpcContract, IpcEvent};
use stablehal_ipc::IpcConfig;
use stablehal_modbus::contract::RtuContract;
use stablehal_modbus::timing::RtuTiming;
use stablehal_shc1::{parse_stream, parse_stream_ipc, peek_contract_code, Header};
use stablehal_shc1::{CONTRACT_IPC_V0_1, CONTRACT_RTU_V0_1};

use stablehal_certify::arg_value;

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

/// Parse `--profile` into an [`IpcConfig`]; defaults to
/// [`IpcConfig::real_socket_local`] — a real capture's `IPC-DELIVERY`
/// samples reflect a genuine kernel round trip between two OS processes,
/// which routinely costs more than `local_non_realtime`'s in-sim-queue
/// bound admits (see that profile's doc comment).
fn ipc_profile(args: &[String]) -> IpcConfig {
    match arg_value(args, "--profile").as_deref() {
        Some("local-non-realtime") => IpcConfig::local_non_realtime(),
        Some("local-realtime") => IpcConfig::local_realtime(),
        Some("cross-host") => IpcConfig::cross_host(),
        _ => IpcConfig::real_socket_local(),
    }
}

fn run_rtu(path: &str, bytes: &[u8], args: &[String]) -> ExitCode {
    let timeout_ms: u64 = arg_value(args, "--timeout-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let json_out = arg_value(args, "--json");

    let (header, events, report) = match parse_stream(bytes) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("failed to parse SHC1 stream {path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };

    let timing = RtuTiming::from_uart(&header.uart_cfg, Nanos::from_millis(timeout_ms));
    let contract = RtuContract::new(timing);
    let verdict = check_trace(contract, &events);

    let trace_hash = stablehal_contract::trace::trace_hash(events.iter());
    let (verdict_str, violation_json) = verdict_json(&verdict);

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

    emit(
        &certificate,
        json_out.as_deref(),
        matches!(verdict, Verdict::Conforming { .. }),
    )
}

fn run_ipc(path: &str, bytes: &[u8], args: &[String]) -> ExitCode {
    let json_out = arg_value(args, "--json");
    let config = ipc_profile(args);

    let (_header, mut events, report) = match parse_stream_ipc(bytes) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("failed to parse SHC1-IPC stream {path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    // A capture that ends mid-run (process killed, trailing EOO lost) still
    // deserves an honest verdict rather than a parse-time hang — the same
    // role `append_end_of_observation` plays for RTU.
    if !matches!(
        events.last().map(|e| &e.event),
        Some(IpcEvent::EndOfObservation)
    ) {
        let at = events
            .last()
            .map(|e| Nanos::from_nanos(e.at.as_nanos() + 1))
            .unwrap_or(Nanos::from_nanos(0));
        events.push(stablehal_contract::TraceEvent {
            at,
            event: IpcEvent::EndOfObservation,
        });
    }

    let verdict = check_trace(IpcContract::new(config), &events);
    let trace_hash = stablehal_contract::trace::trace_hash(events.iter());
    let (verdict_str, violation_json) = verdict_json(&verdict);

    let certificate = format!(
        "{{\n  \"format\": \"stablehal-certificate-v1\",\n  \"contract\": {{ \"id\": \"ipc\", \"version\": \"0.1\", \"clauses\": 9 }},\n  \"capture\": {{ \"records\": {}, \"gaps\": {}, \"trace_hash\": \"{:016x}\" }},\n  \"verdict\": \"{}\",\n  \"violation\": {}\n}}\n",
        report.records,
        report.capture_gaps,
        trace_hash,
        verdict_str,
        violation_json,
    );

    emit(
        &certificate,
        json_out.as_deref(),
        matches!(verdict, Verdict::Conforming { .. }),
    )
}

fn verdict_json<E: Clone + std::fmt::Debug>(verdict: &Verdict<E>) -> (String, String) {
    match verdict {
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
    }
}

fn emit(certificate: &str, json_out: Option<&str>, conforming: bool) -> ExitCode {
    print!("{certificate}");
    if let Some(out_path) = json_out {
        if let Err(e) = fs::write(out_path, certificate) {
            eprintln!("failed to write {out_path}: {e}");
            return ExitCode::FAILURE;
        }
    }
    if conforming {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!(
            "usage: stablehal-certify judge <capture.shc1> --timeout-ms <ms> [--json <report.json>]"
        );
        return ExitCode::FAILURE;
    };

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    match peek_contract_code(&bytes) {
        Ok(CONTRACT_RTU_V0_1) => run_rtu(path, &bytes, args),
        Ok(CONTRACT_IPC_V0_1) => run_ipc(path, &bytes, args),
        Ok(other) => {
            eprintln!("unrecognized contract code {other:#06x} in {path}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("failed to read SHC1 header of {path}: {e:?}");
            ExitCode::FAILURE
        }
    }
}
