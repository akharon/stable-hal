//! `stablehal-certify merge-ipc <requester.shcipc> <responder.shcipc> --out <merged.shcipc>`
//!
//! Merges one requester-side and one responder-side SHC1-IPC capture into a
//! single time-ordered `.shcipc` stream that `judge`/`measure` can then
//! process like any other capture. See `merge_ipc` for why this step exists
//! and its local-host-only scope.

use std::fs;
use std::process::ExitCode;

use stablehal_shc1::{parse_stream_ipc, write_stream_ipc, IpcHeader};

use stablehal_certify::arg_value;
use stablehal_certify::merge_ipc::merge_ipc_traces;

pub fn run(args: &[String]) -> ExitCode {
    let paths: Vec<&String> = args.iter().take_while(|a| !a.starts_with("--")).collect();
    let [path_a, path_b] = paths.as_slice() else {
        eprintln!("usage: stablehal-certify merge-ipc <a.shcipc> <b.shcipc> --out <merged.shcipc>");
        return ExitCode::FAILURE;
    };
    let Some(out_path) = arg_value(args, "--out") else {
        eprintln!("missing --out <merged.shcipc>");
        return ExitCode::FAILURE;
    };

    let (header_a, trace_a) = match read_capture(path_a) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let (header_b, trace_b) = match read_capture(path_b) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let merged = match merge_ipc_traces((header_a, trace_a), (header_b, trace_b)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("failed to merge {path_a} and {path_b}: {e}");
            return ExitCode::FAILURE;
        }
    };

    // The merged stream carries the contract code but neither side's
    // provenance bit alone — it is now a whole-channel trace, not a
    // one-sided capture.
    let merged_header = IpcHeader {
        contract: header_a.contract,
        flags: 0,
    };
    let bytes = write_stream_ipc(&merged_header, &merged);
    if let Err(e) = fs::write(&out_path, &bytes) {
        eprintln!("failed to write {out_path}: {e}");
        return ExitCode::FAILURE;
    }
    println!(
        "merged {} + {} -> {out_path} ({} events)",
        path_a,
        path_b,
        merged.len()
    );
    ExitCode::SUCCESS
}

fn read_capture(
    path: &str,
) -> Result<
    (
        IpcHeader,
        Vec<stablehal_contract::TraceEvent<stablehal_ipc::contract::IpcEvent>>,
    ),
    String,
> {
    let bytes = fs::read(path).map_err(|e| format!("failed to read {path}: {e}"))?;
    let (header, events, _report) =
        parse_stream_ipc(&bytes).map_err(|e| format!("failed to parse SHC1-IPC {path}: {e:?}"))?;
    Ok((header, events))
}
