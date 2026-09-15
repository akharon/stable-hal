//! `stablehal-certify measure capture1.shc1 [capture2.shc1 ...] --timeout-ms N --json out.json`
//!
//! Parses each SHC1 capture, dispatches on its `contract` header field to
//! either the RTU or IPC oracle, and judges it while folding every timing
//! measurement into an envelope hull (min/max per clause, across all input
//! captures). A VIOLATED capture aborts the whole run — an envelope built
//! from a non-conforming capture would misrepresent what "conforms" covers.
//! Mixed-contract input (some RTU, some IPC captures in one invocation) is
//! rejected: an envelope is only meaningful for one contract at a time.

use std::fs;
use std::process::ExitCode;

use stablehal_contract::oracle::check_trace_measured;
use stablehal_contract::{EnvelopeAccumulator, Nanos, Verdict};
use stablehal_ipc::contract::{IpcContract, IpcEvent};
use stablehal_ipc::IpcConfig;
use stablehal_modbus::contract::RtuContract;
use stablehal_modbus::timing::RtuTiming;
use stablehal_shc1::{parse_stream, parse_stream_ipc};
use stablehal_shc1::{CONTRACT_IPC_V0_1, CONTRACT_RTU_V0_1};

use stablehal_certify::arg_value;
use stablehal_certify::envelope::Envelope;

const CAPACITY: usize = 8;
const IPC_CAPACITY: usize = 4;

/// See `judge::ipc_profile`'s doc comment for why the default is
/// `real_socket_local` rather than `local_non_realtime`.
fn ipc_profile(args: &[String]) -> IpcConfig {
    match arg_value(args, "--profile").as_deref() {
        Some("local-non-realtime") => IpcConfig::local_non_realtime(),
        Some("local-realtime") => IpcConfig::local_realtime(),
        Some("cross-host") => IpcConfig::cross_host(),
        _ => IpcConfig::real_socket_local(),
    }
}

fn measure_rtu(paths: &[&String], timeout_ms: u64) -> Result<Envelope, String> {
    let mut acc: EnvelopeAccumulator<CAPACITY> = EnvelopeAccumulator::new();
    let mut traces = 0u64;
    for path in paths {
        let bytes = fs::read(path.as_str()).map_err(|e| format!("failed to read {path}: {e}"))?;
        let (header, events, _report) = parse_stream(&bytes)
            .map_err(|e| format!("failed to parse SHC1 stream {path}: {e:?}"))?;
        let timing = RtuTiming::from_uart(&header.uart_cfg, Nanos::from_millis(timeout_ms));
        let contract = RtuContract::new(timing);
        let (verdict, run_acc) = check_trace_measured::<_, CAPACITY>(contract, &events);
        if let Verdict::Violated(v) = &verdict {
            return Err(format!(
                "capture {path} is not conforming, refusing to build an envelope from it:\n{v}"
            ));
        }
        acc.merge(&run_acc);
        traces += 1;
    }
    Ok(Envelope::from_accumulator(
        "modbus-rtu",
        "capture",
        traces,
        &acc,
    ))
}

fn measure_ipc(paths: &[&String], config: IpcConfig) -> Result<Envelope, String> {
    let mut acc: EnvelopeAccumulator<IPC_CAPACITY> = EnvelopeAccumulator::new();
    let mut traces = 0u64;
    for path in paths {
        let bytes = fs::read(path.as_str()).map_err(|e| format!("failed to read {path}: {e}"))?;
        let (_header, mut events, _report) = parse_stream_ipc(&bytes)
            .map_err(|e| format!("failed to parse SHC1-IPC stream {path}: {e:?}"))?;
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
        let (verdict, run_acc) =
            check_trace_measured::<_, IPC_CAPACITY>(IpcContract::new(config), &events);
        if let Verdict::Violated(v) = &verdict {
            return Err(format!(
                "capture {path} is not conforming, refusing to build an envelope from it:\n{v}"
            ));
        }
        acc.merge(&run_acc);
        traces += 1;
    }
    Ok(Envelope::from_accumulator("ipc", "capture", traces, &acc))
}

pub fn run(args: &[String]) -> ExitCode {
    let paths: Vec<&String> = args.iter().take_while(|a| !a.starts_with("--")).collect();
    if paths.is_empty() {
        eprintln!(
            "usage: stablehal-certify measure <capture.shc1>... --timeout-ms <ms> [--profile <p>] --json <envelope.json>"
        );
        return ExitCode::FAILURE;
    }
    let timeout_ms: u64 = arg_value(args, "--timeout-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let json_out = arg_value(args, "--json");

    let first_bytes = match fs::read(paths[0].as_str()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read {}: {e}", paths[0]);
            return ExitCode::FAILURE;
        }
    };
    let contract = match stablehal_shc1::peek_contract_code(&first_bytes) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read SHC1 header of {}: {e:?}", paths[0]);
            return ExitCode::FAILURE;
        }
    };

    let result = match contract {
        CONTRACT_RTU_V0_1 => measure_rtu(&paths, timeout_ms),
        CONTRACT_IPC_V0_1 => measure_ipc(&paths, ipc_profile(args)),
        other => Err(format!("unrecognized contract code {other:#06x}")),
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
