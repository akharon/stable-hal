//! Real-socket transport for `stablehal-ipc`, capturing an `IpcEvent` trace
//! as it runs — the software analogue of a wire pilot's hardware capture
//! (`next_steps.md` item 2).
//!
//! `stablehal-ipc`'s reference driver ([`stablehal_ipc::ScriptedRequester`])
//! and reference responder ([`stablehal_ipc::IpcResponderDevice`]) are
//! written purely against the [`stablehal_ipc::IpcRequester`]/
//! [`stablehal_ipc::IpcResponder`] traits — they have no idea whether the
//! port underneath is `stablehal_sim`'s in-memory queue or a real socket.
//! [`RealSocketRequester`] and [`RealSocketResponder`] implement those same
//! traits over a real `AF_UNIX SOCK_DGRAM` socket, so the *unmodified*
//! reference driver/device can be pointed at a real channel with no code of
//! their own knowing the difference. The capture instrumentation
//! (timestamping + `IpcEvent` recording) lives directly in these port
//! types, alongside the real `send`/`recv` calls they already make — no
//! `unsafe`, no interposition, pure `std`.
//!
//! # Wire framing
//!
//! One `SOCK_DGRAM` datagram carries one [`stablehal_ipc::Datagram`]:
//! `id: u32 LE | len: u16 LE | payload: [u8; len]` (`len <= MAX_PAYLOAD`).
//! `SOCK_DGRAM` preserves message boundaries, so no length-prefixed
//! streaming framing is needed beyond this. The requester side must
//! `bind()` its own socket to a filesystem path before `connect()`-ing:
//! an unbound `AF_UNIX` datagram socket has no addressable identity, so the
//! responder (which replies via `send_to` the address a request arrived
//! from) would otherwise have nowhere to send the response — see
//! [`RealSocketResponder`]'s doc comment.
//!
//! # What is captured, and what is not
//!
//! * `Sent`/`ResponseDelivered` are recorded on the requester port;
//!   `Delivered`/`Responded` on the responder port. `SendRejected` is
//!   recorded by the requester port whenever the OS reports the send
//!   would block — this is a channel-level observation the port makes on
//!   the driver's behalf, not something `IpcRequester`'s trait contract
//!   asks the driver itself to report.
//! * `SilentDrop` is **never emitted live** — it is only inferable by
//!   absence, exactly as `IpcContract::step_measured` already infers an
//!   unanswered transaction at `EndOfObservation`. Declaring this rather
//!   than working around it matches the project's existing "state the
//!   limit plainly" pattern (e.g. `Truncated` in the wire pilots).
//! * Peer death (`recv` returning `Ok(0)` or `ECONNRESET`) records
//!   `Fault{PeerGone}` then `PeerExit`.
//! * Scope: one host, one requester, one responder. Both sides must record
//!   against the **same shared origin** (a `SystemTime` computed once by
//!   whatever spawns them, passed to both — see [`Capture::new`]) so their
//!   captures can be merged without clock-skew correction (see
//!   `stablehal-certify`'s `merge_ipc`). Cross-host capture is out of
//!   scope for this pilot.

use std::io::ErrorKind;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::SystemTime;

use stablehal_contract::{Nanos, TraceEvent};
use stablehal_ipc::contract::IpcEvent;
use stablehal_ipc::{CorrelationId, Datagram, IpcFault, IpcRequester, IpcResponder, IpcSendError};

/// Wire-encode a [`Datagram`]: `id: u32 LE | len: u16 LE | payload`.
fn encode(msg: &Datagram) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + msg.len as usize);
    out.extend_from_slice(&msg.id.to_le_bytes());
    out.extend_from_slice(&(msg.len as u16).to_le_bytes());
    out.extend_from_slice(msg.payload());
    out
}

/// Decode a wire-framed [`Datagram`]. `None` on a malformed/short buffer.
fn decode(buf: &[u8]) -> Option<Datagram> {
    if buf.len() < 6 {
        return None;
    }
    let id = CorrelationId::from_le_bytes(buf[0..4].try_into().unwrap());
    let len = u16::from_le_bytes(buf[4..6].try_into().unwrap()) as usize;
    let payload = buf.get(6..6 + len)?;
    Some(Datagram::new(id, payload))
}

/// Records `IpcEvent`s against an explicit shared clock origin, and can
/// serialize the resulting trace as an SHC1-IPC byte stream.
///
/// The origin must be the **same wall-clock instant across both processes**
/// — `std::time::Instant` cannot serve this role: even though it is backed
/// by `CLOCK_MONOTONIC` on Linux, safe `std` gives no way to read its raw
/// value or share it between processes, and each process's own
/// `Instant::now()` only orders against other `Instant`s *in that same
/// process*. So the caller computes one `SystemTime` before spawning both
/// sides (e.g. the test harness, or a small launcher) and passes it to
/// both — see the two fixture binaries for how. `SystemTime` can in
/// principle jump on an NTP step, which `CLOCK_MONOTONIC` cannot; that
/// tradeoff is accepted here in exchange for staying in safe `std` with no
/// `libc` dependency (G1 restricts external deps in `crates/*`, not
/// `tools/*`, but this design needs none either way).
pub struct Capture {
    origin: SystemTime,
    trace: Vec<TraceEvent<IpcEvent>>,
}

impl Capture {
    pub fn new(origin: SystemTime) -> Self {
        Capture {
            origin,
            trace: Vec::new(),
        }
    }

    fn record(&mut self, event: IpcEvent) {
        let elapsed = SystemTime::now()
            .duration_since(self.origin)
            .unwrap_or_default();
        let at = Nanos::from_nanos(elapsed.as_nanos() as u64);
        self.trace.push(TraceEvent { at, event });
    }

    /// Flush a trailing `PeerExit` + `EndOfObservation` — call once when the
    /// driving loop is done, whether because the script finished or the
    /// peer went away.
    pub fn finish(&mut self, peer_gone: bool) {
        if peer_gone {
            self.record(IpcEvent::Fault {
                fault: IpcFault::PeerGone,
            });
            self.record(IpcEvent::PeerExit);
        }
        self.record(IpcEvent::EndOfObservation);
    }

    pub fn trace(&self) -> &[TraceEvent<IpcEvent>] {
        &self.trace
    }

    /// Serialize as an SHC1-IPC stream. `responder_side` sets
    /// `FLAG_IPC_RESPONDER_SIDE` so `stablehal-certify merge-ipc` can tell
    /// the two captures apart.
    pub fn to_shc1(&self, responder_side: bool) -> Vec<u8> {
        let header = stablehal_shc1::IpcHeader {
            contract: stablehal_shc1::CONTRACT_IPC_V0_1,
            flags: if responder_side {
                stablehal_shc1::FLAG_IPC_RESPONDER_SIDE
            } else {
                0
            },
        };
        stablehal_shc1::write_stream_ipc(&header, &self.trace)
    }
}

/// Requester-side real-socket port: implements [`IpcRequester`] over a
/// connected, non-blocking datagram socket.
pub struct RealSocketRequester {
    sock: UnixDatagram,
    capture: Capture,
    peer_gone: bool,
}

impl RealSocketRequester {
    pub fn new(sock: UnixDatagram, capture_origin: SystemTime) -> std::io::Result<Self> {
        sock.set_nonblocking(true)?;
        Ok(RealSocketRequester {
            sock,
            capture: Capture::new(capture_origin),
            peer_gone: false,
        })
    }

    /// Consume `self`, flushing the trailing observation records.
    pub fn finish(mut self) -> Capture {
        self.capture.finish(self.peer_gone);
        self.capture
    }
}

impl IpcRequester for RealSocketRequester {
    fn try_send(&mut self, msg: &Datagram) -> Result<(), IpcSendError> {
        let bytes = encode(msg);
        match self.sock.send(&bytes) {
            Ok(_) => {
                self.capture.record(IpcEvent::Sent {
                    id: msg.id,
                    len: msg.len as u32,
                });
                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                self.capture.record(IpcEvent::SendRejected { id: msg.id });
                Err(IpcSendError::WouldBlock)
            }
            Err(e) if is_peer_gone(&e) => {
                self.peer_gone = true;
                Err(IpcSendError::PeerGone)
            }
            Err(e) => panic!("unexpected socket send error: {e}"),
        }
    }

    fn try_recv(&mut self) -> Option<Result<Datagram, IpcFault>> {
        let mut buf = [0u8; 512];
        match self.sock.recv(&mut buf) {
            Ok(0) => {
                self.peer_gone = true;
                Some(Err(IpcFault::PeerGone))
            }
            Ok(n) => {
                let msg = decode(&buf[..n])?;
                self.capture
                    .record(IpcEvent::ResponseDelivered { id: msg.id });
                Some(Ok(msg))
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => None,
            Err(e) if is_peer_gone(&e) => {
                self.peer_gone = true;
                Some(Err(IpcFault::PeerGone))
            }
            Err(e) => panic!("unexpected socket recv error: {e}"),
        }
    }
}

/// Responder-side real-socket port. Unlike `IpcRequester`, `IpcResponder`
/// has no polling method of its own (`on_request` is called by the driving
/// loop) — so this type exposes a `poll` method the fixture binary's loop
/// calls directly, rather than implementing `IpcResponder` itself.
///
/// A responder's socket is *bound*, not *connected* — `AF_UNIX SOCK_DGRAM`
/// has no accept/connection step on the passive side. So requests are read
/// with `recv_from` (which also yields the caller's address) and answered
/// with `send_to` that same address, rather than the connected `send`/
/// `recv` the requester side can use once it has `connect()`-ed.
pub struct RealSocketResponder {
    sock: UnixDatagram,
    capture: Capture,
    peer_gone: bool,
    peer_addr: Option<SocketAddr>,
}

impl RealSocketResponder {
    pub fn new(sock: UnixDatagram, capture_origin: SystemTime) -> std::io::Result<Self> {
        sock.set_nonblocking(true)?;
        Ok(RealSocketResponder {
            sock,
            capture: Capture::new(capture_origin),
            peer_gone: false,
            peer_addr: None,
        })
    }

    pub fn peer_gone(&self) -> bool {
        self.peer_gone
    }

    pub fn finish(mut self) -> Capture {
        self.capture.finish(self.peer_gone);
        self.capture
    }

    /// Non-blocking: receive one request (if any), hand it to `responder`,
    /// and send back whatever it returns. Returns `true` if a request was
    /// processed (caller should poll again promptly).
    ///
    /// `pre_response_delay`, if set, is applied after a request is
    /// delivered but before the response is sent — modeling an injected
    /// scheduling stall so it lands inside the `IPC-RESP-TIME` window
    /// rather than in the idle-polling gaps between exchanges.
    pub fn poll(
        &mut self,
        responder: &mut dyn IpcResponder,
        pre_response_delay: Option<std::time::Duration>,
    ) -> bool {
        let mut buf = [0u8; 512];
        match self.sock.recv_from(&mut buf) {
            Ok((0, _)) => {
                self.peer_gone = true;
                responder.on_peer_gone();
                true
            }
            Ok((n, from)) => {
                self.peer_addr = Some(from);
                let Some(req) = decode(&buf[..n]) else {
                    return true;
                };
                self.capture.record(IpcEvent::Delivered { id: req.id });
                if let Some(d) = pre_response_delay {
                    std::thread::sleep(d);
                }
                if let Some(resp) = responder.on_request(&req) {
                    let bytes = encode(&resp);
                    let send_result = match &self.peer_addr {
                        Some(addr) => self.sock.send_to_addr(&bytes, addr),
                        None => unreachable!("peer_addr was just set from recv_from"),
                    };
                    match send_result {
                        Ok(_) => {
                            self.capture.record(IpcEvent::Responded {
                                id: resp.id,
                                len: resp.len as u32,
                            });
                        }
                        Err(e) if is_peer_gone(&e) => {
                            self.peer_gone = true;
                            responder.on_peer_gone();
                        }
                        Err(e) => panic!("unexpected socket send error: {e}"),
                    }
                }
                true
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => false,
            Err(e) if is_peer_gone(&e) => {
                self.peer_gone = true;
                responder.on_peer_gone();
                true
            }
            Err(e) => panic!("unexpected socket recv error: {e}"),
        }
    }
}

fn is_peer_gone(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::ConnectionReset | ErrorKind::BrokenPipe | ErrorKind::NotConnected
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagram_wire_format_round_trips() {
        let msg = Datagram::new(0x1234_5678, &[1, 2, 3, 4, 5]);
        let bytes = encode(&msg);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn empty_payload_round_trips() {
        let msg = Datagram::new(1, &[]);
        let decoded = decode(&encode(&msg)).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn short_buffer_fails_to_decode() {
        assert!(decode(&[1, 2, 3]).is_none());
    }
}
