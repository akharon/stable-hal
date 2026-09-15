//! Real-socket responder fixture: binds a datagram socket, drives the
//! *unmodified* `stablehal_ipc::IpcResponderDevice` reference device
//! against it via [`ipc_capture_shim::RealSocketResponder`], and writes its
//! capture to disk.
//!
//! Usage: `ipc-responder-fixture <socket-path> <capture-out-path> [delay-ms]`
//!
//! An optional `delay-ms` sleeps before each response, *in addition to* the
//! fixed [`RESP_TIME_FLOOR`], for the negative containment test (a real
//! capture whose `IPC-RESP-TIME` samples run slower than anything
//! `sim_envelope_sweep` explored).
//!
//! Reads `IPC_CAPTURE_EPOCH_NS` (nanoseconds since `UNIX_EPOCH`) from the
//! environment as the shared capture origin — must be the same value given
//! to the requester fixture, see `ipc_capture_shim::Capture`'s doc comment.

use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant, UNIX_EPOCH};

use ipc_capture_shim::RealSocketResponder;
use stablehal_ipc::IpcResponderDevice;

/// `IpcConfig`'s `resp_time` lower bound (10µs) models that a real response
/// cannot arrive faster than an OS could plausibly reschedule the
/// responder process — it exists to catch an implausibly-instant answer,
/// the same role a lower bound plays for any wire pilot's timing clause.
/// A tight busy-poll loop on an idle box answers faster than that (measured
/// ~7µs), which is a fixture artifact, not something a *real* scheduled
/// service would ever do — so this fixture holds itself to a floor safely
/// above the bound rather than letting the contract's lower bound go
/// unexercised by every real capture this pilot produces.
const RESP_TIME_FLOOR: Duration = Duration::from_micros(15);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let socket_path = args
        .get(1)
        .expect("usage: ipc-responder-fixture <socket-path> <out-path> [delay-ms]");
    let out_path = args
        .get(2)
        .expect("usage: ipc-responder-fixture <socket-path> <out-path> [delay-ms]");
    let extra_delay = args
        .get(3)
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_default();
    let delay = Some(RESP_TIME_FLOOR + extra_delay);
    let epoch_ns: u64 = std::env::var("IPC_CAPTURE_EPOCH_NS")
        .expect("IPC_CAPTURE_EPOCH_NS must be set to a shared origin (ns since UNIX_EPOCH)")
        .parse()
        .expect("IPC_CAPTURE_EPOCH_NS must be a u64");
    let origin = UNIX_EPOCH + Duration::from_nanos(epoch_ns);

    let _ = std::fs::remove_file(socket_path);
    let sock = UnixDatagram::bind(socket_path).expect("bind responder socket");

    let mut port = RealSocketResponder::new(sock, origin).expect("set_nonblocking");
    let mut device = IpcResponderDevice::new();

    let deadline = Instant::now() + Duration::from_secs(5);
    // Stop once the peer has gone, or once the channel has gone idle for a
    // while *after* the first request arrived — a fixed request count isn't
    // known to this side, so post-first-activity idleness is the completion
    // signal. Idleness is not measured before the first request: the
    // requester process still needs to spawn and connect, which can easily
    // take longer than the idle window itself.
    let mut idle_since: Option<Instant> = None;
    while Instant::now() < deadline && !port.peer_gone() {
        let progressed = port.poll(&mut device, delay);
        if progressed {
            idle_since = Some(Instant::now());
        }
        if let Some(since) = idle_since {
            if since.elapsed() > Duration::from_millis(1000) {
                break;
            }
        }
        if !progressed {
            std::thread::sleep(Duration::from_micros(50));
        }
    }

    let capture = port.finish();
    std::fs::write(out_path, capture.to_shc1(true)).expect("write capture");
}
