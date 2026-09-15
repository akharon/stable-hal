//! In-process proof that the IPC capture path works end to end through the
//! `stablehal-certify` binary's `judge`/`measure`/`merge-ipc` surface,
//! without needing a real socket: builds two `.shcipc` byte streams (one
//! per side) directly via `write_stream_ipc`, exercises `merge_ipc`, and
//! shells out to the built `stablehal-certify` binary for `judge`/`measure`.
//!
//! The real-socket acceptance-bar proof lives in
//! `tools/ipc-capture-shim`'s own tests; this file is the fast,
//! deterministic complement that isolates the format/CLI plumbing.

use std::path::PathBuf;
use std::process::Command;

use stablehal_contract::{Nanos, TraceEvent};
use stablehal_ipc::contract::IpcEvent;
use stablehal_shc1::{write_stream_ipc, IpcHeader, CONTRACT_IPC_V0_1, FLAG_IPC_RESPONDER_SIDE};

fn bin() -> PathBuf {
    // Standard cargo integration-test convention: the built binary sits
    // next to the test harness in target/<profile>/.
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps/
    path.pop(); // <profile>/
    path.push("stablehal-certify");
    path
}

fn write(
    dir: &std::path::Path,
    name: &str,
    header: &IpcHeader,
    trace: &[TraceEvent<IpcEvent>],
) -> PathBuf {
    let bytes = write_stream_ipc(header, trace);
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn requester_header() -> IpcHeader {
    IpcHeader {
        contract: CONTRACT_IPC_V0_1,
        flags: 0,
    }
}

fn responder_header() -> IpcHeader {
    IpcHeader {
        contract: CONTRACT_IPC_V0_1,
        flags: FLAG_IPC_RESPONDER_SIDE,
    }
}

fn conforming_sides() -> (Vec<TraceEvent<IpcEvent>>, Vec<TraceEvent<IpcEvent>>) {
    let requester = vec![
        TraceEvent {
            at: Nanos::from_micros(10),
            event: IpcEvent::Sent { id: 1, len: 4 },
        },
        TraceEvent {
            at: Nanos::from_micros(62),
            event: IpcEvent::ResponseDelivered { id: 1 },
        },
    ];
    let responder = vec![
        TraceEvent {
            at: Nanos::from_micros(11),
            event: IpcEvent::Delivered { id: 1 },
        },
        TraceEvent {
            at: Nanos::from_micros(61),
            event: IpcEvent::Responded { id: 1, len: 4 },
        },
    ];
    (requester, responder)
}

#[test]
fn merge_then_judge_a_conforming_pair_of_captures_is_certified_conforming() {
    let dir = tempdir();
    let (requester, responder) = conforming_sides();
    let req_path = write(&dir, "requester.shcipc", &requester_header(), &requester);
    let resp_path = write(&dir, "responder.shcipc", &responder_header(), &responder);
    let merged_path = dir.join("merged.shcipc");

    let status = Command::new(bin())
        .args([
            "merge-ipc",
            req_path.to_str().unwrap(),
            resp_path.to_str().unwrap(),
            "--out",
            merged_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "merge-ipc failed");

    let output = Command::new(bin())
        .args(["judge", merged_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "judge failed:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("\"verdict\": \"CONFORMING\""), "{text}");
}

#[test]
fn merge_then_judge_a_dropped_response_is_flagged_at_resp_time() {
    let dir = tempdir();
    let requester = vec![
        TraceEvent {
            at: Nanos::from_micros(10),
            event: IpcEvent::Sent { id: 1, len: 4 },
        },
        // No ResponseDelivered: the responder never answers.
    ];
    let responder = vec![TraceEvent {
        at: Nanos::from_micros(11),
        event: IpcEvent::Delivered { id: 1 },
    }];
    let req_path = write(&dir, "requester.shcipc", &requester_header(), &requester);
    let resp_path = write(&dir, "responder.shcipc", &responder_header(), &responder);
    let merged_path = dir.join("merged.shcipc");

    let status = Command::new(bin())
        .args([
            "merge-ipc",
            req_path.to_str().unwrap(),
            resp_path.to_str().unwrap(),
            "--out",
            merged_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "merge-ipc failed");

    let output = Command::new(bin())
        .args(["judge", merged_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "judge should have failed");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("IPC-RESP-TIME"), "{text}");
}

#[test]
fn measure_on_a_conforming_capture_emits_an_ipc_envelope() {
    let dir = tempdir();
    let (requester, responder) = conforming_sides();
    let req_path = write(&dir, "requester.shcipc", &requester_header(), &requester);
    let resp_path = write(&dir, "responder.shcipc", &responder_header(), &responder);
    let merged_path = dir.join("merged.shcipc");

    Command::new(bin())
        .args([
            "merge-ipc",
            req_path.to_str().unwrap(),
            resp_path.to_str().unwrap(),
            "--out",
            merged_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    let output = Command::new(bin())
        .args(["measure", merged_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "measure failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("\"contract\": \"ipc\""), "{text}");
    assert!(text.contains("IPC-RESP-TIME"), "{text}");
    assert!(text.contains("IPC-DELIVERY"), "{text}");
}

fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "stablehal-certify-ipc-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
