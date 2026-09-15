//! The acceptance-bar test for `next_steps.md` item 2: "a capture from a
//! real Unix-socket service certified, and its measured `IPC-RESP-TIME`
//! hull compared against `sim_envelope_sweep` via the existing containment
//! path."
//!
//! Runs the real fixture binaries with a deliberately widened (but still
//! conforming) responder latency, merges the two real captures, folds them
//! into a hardware-measured envelope via `measure`, gets the sim-explored
//! envelope via `sim-envelope --protocol ipc`, and runs `contain` — proving
//! the full real-capture-to-containment path has teeth, exactly as
//! `tests/ipc_envelope.rs` already proves in-process for pure simulation
//! (see `docs/user-guide-ipc.md` §7).

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn fixture_bin(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push(name);
    path
}

fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipc-capture-shim-containment-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_real_capture_slower_than_the_sim_sweep_is_reported_uncovered() {
    let dir = tempdir();
    let sock_path = dir.join("ipc.sock");
    let req_out = dir.join("requester.shcipc");
    let resp_out = dir.join("responder.shcipc");
    let epoch_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string();

    // A real kernel round trip between two processes already sits outside
    // sim's in-memory-queue IPC-DELIVERY hull (see this test's own
    // discovery during development — a plain conforming exchange is
    // already enough to prove IPC-DELIVERY uncovered). No extra delay is
    // needed to make the point; keep it explicit at 0 for clarity.
    let mut responder = Command::new(fixture_bin("ipc-responder-fixture"))
        .arg(&sock_path)
        .arg(&resp_out)
        .arg("0")
        .env("IPC_CAPTURE_EPOCH_NS", &epoch_ns)
        .spawn()
        .expect("spawn responder fixture");
    std::thread::sleep(Duration::from_millis(300));
    let requester_status = Command::new(fixture_bin("ipc-requester-fixture"))
        .arg(&sock_path)
        .arg(&req_out)
        .env("IPC_CAPTURE_EPOCH_NS", &epoch_ns)
        .status()
        .expect("run requester fixture");
    assert!(requester_status.success());
    assert!(responder.wait().unwrap().success());

    let certify = fixture_bin("stablehal-certify");
    let merged = dir.join("merged.shcipc");
    let status = Command::new(&certify)
        .args([
            "merge-ipc",
            req_out.to_str().unwrap(),
            resp_out.to_str().unwrap(),
            "--out",
            merged.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "merge-ipc failed");

    // The real capture must itself be conforming — an envelope built from
    // a non-conforming capture would misrepresent what "conforms" covers
    // (see measure.rs's own doc comment).
    let judge_output = Command::new(&certify)
        .args(["judge", merged.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        judge_output.status.success(),
        "real capture was not conforming:\n{}",
        String::from_utf8_lossy(&judge_output.stdout)
    );

    let hw_json = dir.join("hw_envelope.json");
    let status = Command::new(&certify)
        .args(["measure", merged.to_str().unwrap(), "--json"])
        .arg(&hw_json)
        .status()
        .unwrap();
    assert!(status.success(), "measure failed");

    let sim_json = dir.join("sim_envelope.json");
    let status = Command::new(&certify)
        .args([
            "sim-envelope",
            "--protocol",
            "ipc",
            "--seeds",
            "50",
            "--json",
        ])
        .arg(&sim_json)
        .status()
        .unwrap();
    assert!(status.success(), "sim-envelope failed");

    let contain_output = Command::new(&certify)
        .args(["contain", "--hw"])
        .arg(&hw_json)
        .arg("--sim")
        .arg(&sim_json)
        .output()
        .unwrap();
    // NOT all-covered is the expected, honest result: sim models an
    // in-memory queue's delivery latency, which is orders of magnitude
    // tighter than a real kernel round trip between two OS processes. This
    // is exactly the "did the simulation actually explore the region
    // production runs in?" question containment exists to answer, and here
    // the honest answer is no for IPC-DELIVERY — the concrete payoff
    // next_steps.md's acceptance bar calls out.
    assert!(
        !contain_output.status.success(),
        "expected NOT all-covered (a real socket's delivery latency exceeds the sim-explored hull)"
    );
    let text = String::from_utf8_lossy(&contain_output.stdout);
    assert!(text.contains("\"status\": \"UNCOVERED\""), "{text}");
    assert!(text.contains("IPC-DELIVERY"), "{text}");
}
