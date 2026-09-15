//! Real-socket requester fixture: connects to a datagram socket, drives the
//! *unmodified* `stablehal_ipc::ScriptedRequester` reference driver against
//! it via [`ipc_capture_shim::RealSocketRequester`], and writes its capture
//! to disk.
//!
//! Usage: `ipc-requester-fixture <socket-path> <capture-out-path>`
//!
//! Reads `IPC_CAPTURE_EPOCH_NS` (nanoseconds since `UNIX_EPOCH`) from the
//! environment as the shared capture origin — see
//! `ipc_capture_shim::Capture`'s doc comment for why this must be shared
//! with the responder fixture rather than each side timing its own start.

use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant, UNIX_EPOCH};

use ipc_capture_shim::RealSocketRequester;
use stablehal_ipc::{IpcStep, ScriptedRequester};

const SCRIPT: &[IpcStep] = &[
    IpcStep {
        id: 1,
        payload: &[0xAB, 0xCD],
    },
    IpcStep {
        id: 2,
        payload: &[0x11, 0x22],
    },
    IpcStep {
        id: 3,
        payload: &[0x33, 0x44],
    },
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let socket_path = args
        .get(1)
        .expect("usage: ipc-requester-fixture <socket-path> <out-path>");
    let out_path = args
        .get(2)
        .expect("usage: ipc-requester-fixture <socket-path> <out-path>");
    let epoch_ns: u64 = std::env::var("IPC_CAPTURE_EPOCH_NS")
        .expect("IPC_CAPTURE_EPOCH_NS must be set to a shared origin (ns since UNIX_EPOCH)")
        .parse()
        .expect("IPC_CAPTURE_EPOCH_NS must be a u64");
    let origin = UNIX_EPOCH + Duration::from_nanos(epoch_ns);

    let sock = connect_with_retry(socket_path);
    let mut port = RealSocketRequester::new(sock, origin).expect("set_nonblocking");
    let mut driver = ScriptedRequester::new(SCRIPT);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !driver.done() && Instant::now() < deadline {
        if !driver.step(&mut port) {
            std::thread::sleep(Duration::from_micros(50));
        }
    }
    assert!(
        driver.done(),
        "requester script did not complete within the deadline, outcomes={:?}",
        driver.outcomes()
    );

    let capture = port.finish();
    std::fs::write(out_path, capture.to_shc1(false)).expect("write capture");
}

/// Connects to `path`, first binding this side to its own unique path.
///
/// An `AF_UNIX SOCK_DGRAM` client that never binds has no addressable
/// identity: `recv_from` on the passive (responder) side reports such a
/// peer as "unnamed", and `send_to` back to it fails with `EINVAL` — there
/// is nowhere to send to. Binding the requester's own socket first gives
/// the responder a real path to reply to.
fn connect_with_retry(path: &str) -> UnixDatagram {
    let own_path = format!("{path}.client-{}", std::process::id());
    let _ = std::fs::remove_file(&own_path);
    let sock = UnixDatagram::bind(&own_path).expect("bind requester socket");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match sock.connect(path) {
            Ok(()) => return sock,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => panic!("failed to connect to {path}: {e}"),
        }
    }
}
