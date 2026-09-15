//! Spawns the two fixture binaries as real child processes talking over a
//! real `AF_UNIX` datagram socket, then proves the resulting captures parse
//! and judge correctly — the acceptance bar from `next_steps.md` item 2:
//! "a capture from a real Unix-socket service certified."
//!
//! The fast, deterministic complement to these tests (format/CLI plumbing,
//! no real socket) lives in `tools/stablehal-certify/tests/ipc_capture_certify.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn fixture_bin(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps/
    path.pop(); // <profile>/
    path.push(name);
    path
}

fn certify_bin() -> PathBuf {
    fixture_bin("stablehal-certify")
}

fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipc-capture-shim-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the responder and requester fixtures against a fresh socket in
/// `dir`, with `responder_extra_delay_ms` passed as the responder's
/// negative-test delay argument (0 = none beyond its own resp-time floor).
/// Returns the paths to the two raw captures.
fn run_exchange(dir: &Path, responder_extra_delay_ms: u64) -> (PathBuf, PathBuf) {
    let sock_path = dir.join("ipc.sock");
    let req_out = dir.join("requester.shcipc");
    let resp_out = dir.join("responder.shcipc");

    let epoch_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string();

    let mut responder = Command::new(fixture_bin("ipc-responder-fixture"))
        .arg(&sock_path)
        .arg(&resp_out)
        .arg(responder_extra_delay_ms.to_string())
        .env("IPC_CAPTURE_EPOCH_NS", &epoch_ns)
        .spawn()
        .expect("spawn responder fixture");

    // Give the responder a moment to bind before the requester connects.
    std::thread::sleep(Duration::from_millis(300));

    let requester_status = Command::new(fixture_bin("ipc-requester-fixture"))
        .arg(&sock_path)
        .arg(&req_out)
        .env("IPC_CAPTURE_EPOCH_NS", &epoch_ns)
        .status()
        .expect("run requester fixture");
    assert!(requester_status.success(), "requester fixture failed");

    let responder_status = responder.wait().expect("wait for responder fixture");
    assert!(responder_status.success(), "responder fixture failed");

    (req_out, resp_out)
}

fn merge(dir: &Path, req: &Path, resp: &Path) -> PathBuf {
    let merged = dir.join("merged.shcipc");
    let status = Command::new(certify_bin())
        .args([
            "merge-ipc",
            req.to_str().unwrap(),
            resp.to_str().unwrap(),
            "--out",
            merged.to_str().unwrap(),
        ])
        .status()
        .expect("run merge-ipc");
    assert!(status.success(), "merge-ipc failed");
    merged
}

#[test]
fn a_conforming_real_exchange_is_certified_conforming() {
    let dir = tempdir();
    let (req, resp) = run_exchange(&dir, 0);
    let merged = merge(&dir, &req, &resp);

    let output = Command::new(certify_bin())
        .args(["judge", merged.to_str().unwrap()])
        .output()
        .expect("run judge");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "expected CONFORMING, got:\n{text}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("\"verdict\": \"CONFORMING\""), "{text}");
}

#[test]
fn a_slow_real_responder_is_flagged_at_resp_time() {
    let dir = tempdir();
    // 25ms extra, well past the 20ms upper bound every IpcConfig profile
    // shares for resp_time.
    let (req, resp) = run_exchange(&dir, 25);
    let merged = merge(&dir, &req, &resp);

    let output = Command::new(certify_bin())
        .args(["judge", merged.to_str().unwrap()])
        .output()
        .expect("run judge");
    assert!(!output.status.success(), "expected a violation");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("IPC-RESP-TIME"), "{text}");
}
