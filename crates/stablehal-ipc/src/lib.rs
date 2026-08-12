//! IPC channel contract: a request/response datagram exchange between two
//! processes over a **bounded** queue (the Unix-domain-socket / POSIX
//! message-queue / shared-ring shape).
//!
//! # Why IPC is a contract problem at all
//!
//! The five prior pilots (MODBUS RTU, I2C, LIN, SPI, CAN) are *wire* buses:
//! their nondeterminism is electrical and their timing envelopes come from a
//! clock crystal. IPC on one host between two real-time-scheduled processes
//! has almost none of that — which is exactly why it is usually left
//! untested. But the interesting deployments are the two where bounded
//! nondeterminism is real and large:
//!
//! * **Non-real-time scheduling.** A responder under `SCHED_OTHER` does not
//!   answer in a fixed time; it answers whenever the scheduler next runs it.
//!   That is a genuine `min..=max` envelope (`IPC-RESP-TIME`), typically
//!   spanning two or three orders of magnitude, and it is the single most
//!   common source of "worked on my machine, timed out in production".
//! * **Cross-host transport.** Once the channel leaves the machine, delivery
//!   latency becomes an envelope (`IPC-DELIVERY`) and the fault vocabulary
//!   grows terms a local pipe never has: duplicate delivery and reordering.
//!
//! Both are *bounded* nondeterminism, so both are contractualizable in
//! exactly the way `docs/architecture.md` §4 describes: state the envelope,
//! close the fault set, sample it under a seed, judge it with one oracle.
//!
//! # What is deliberately different from the wire pilots
//!
//! | Concern | Wire buses | This contract |
//! |---|---|---|
//! | Timing origin | crystal / baud rate | OS scheduler + transport |
//! | Envelope width | ±2–5% | often 100× (`min..=max` spans µs to ms) |
//! | Fault vocabulary | framing, parity, noise, overrun | [`IpcFault`]: peer death, backpressure, truncation, duplicate, reorder |
//! | Endpoint liveness | a device is always "there" | a peer can **die mid-transaction** and never come back |
//! | Flow control | none (or clock stretching) | bounded queue ⇒ backpressure is a first-class, *legal* outcome |
//!
//! The last two rows are the ones with no analogue in MODBUS/I2C/LIN/SPI/CAN,
//! and they are why this is a new contract rather than a re-parameterization
//! of an existing one.
//!
//! # Granularity
//!
//! Modeled at **datagram** granularity — one whole message is one event — not
//! at byte or page granularity. This is the level at which IPC software
//! contracts actually live: a caller sends a request and awaits a response;
//! it never observes partial frames the way a UART driver does. Truncation
//! (a short read against a bounded buffer) is therefore represented as an
//! explicit fault term rather than as a byte-level framing state.
//!
//! # Wire events and clauses
//!
//! | Clause | Citation | Obligation |
//! |--------|----------|------------|
//! | IPC-SEQ | IPC channel contract §1 | Request/response strictly alternate per correlation id; no response without a matching outstanding request. |
//! | IPC-RESP-TIME | IPC channel contract §2 | A responder answers within the scheduling-latency envelope (`resp_time`). |
//! | IPC-DELIVERY | IPC channel contract §3 | Delivery latency (send → receive) stays inside the transport envelope. |
//! | IPC-CORRELATION | IPC channel contract §4 | A response carries the correlation id of the request it answers. |
//! | IPC-MSG-MAXLEN | IPC channel contract §5 | No datagram exceeds the channel's configured maximum length. |
//! | IPC-BACKPRESSURE | IPC channel contract §6 | A send into a full queue must fail loudly (`WouldBlock`), never silently drop. |
//! | IPC-NO-DUPLICATE | IPC channel contract §7 | A correlation id is answered at most once (at-most-once delivery). |
//! | IPC-PEER-DEATH | IPC channel contract §8 | Peer death must surface as an explicit `PeerGone` fault to the survivor, not as an infinite hang. |
//! | IPC-ORDERING | IPC channel contract §9 | Datagrams within one correlation stream are observed in send order. |
//!
//! # Judging model
//!
//! Per-direction, exactly as the wire pilots: `judge_requester` /
//! `judge_responder`. A conformance tester deliberately violates its own
//! side's clauses (sends an over-long datagram, answers a correlation id
//! twice) while the DUT stays fully accountable.

#![no_std]
#![deny(unsafe_code)]

pub mod contract;
pub mod device;
pub mod requester;

pub use contract::{IpcContract, IpcEvent, IpcState};
pub use device::{IpcResponderBugs, IpcResponderDevice};
pub use requester::{IpcStep, IpcStepOutcome, ScriptedRequester};

use stablehal_contract::{Nanos, TimeBound};

// ---------------------------------------------------------------------------
// Fault vocabulary (closed)
// ---------------------------------------------------------------------------

/// The **closed** IPC fault set. The environment may inject only these; a
/// conforming endpoint must tolerate exactly these. Mirrors the role
/// `UartError {Framing, Parity, Noise, Overrun}` plays for the wire pilots —
/// but note that none of the four UART terms appear here, and none of the
/// five terms here have a UART analogue. That non-overlap *is* the argument
/// for a separate contract.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IpcFault {
    /// The peer process exited or closed its endpoint. Terminal for the
    /// channel: no further traffic is legal in either direction.
    PeerGone,
    /// The receive queue was full; the send was rejected. **Legal** and
    /// recoverable — the sender is expected to retry or shed load. This is
    /// the one fault that is not a failure.
    WouldBlock,
    /// A datagram arrived truncated (short read against a bounded buffer).
    Truncated,
    /// A datagram was delivered more than once (cross-host transports only).
    Duplicate,
    /// A datagram arrived out of send order (cross-host transports only).
    Reordered,
}

impl IpcFault {
    /// Whether this fault terminates the channel. Only [`IpcFault::PeerGone`]
    /// does; everything else is a recoverable condition the endpoint is
    /// contractually required to survive.
    pub const fn is_terminal(self) -> bool {
        matches!(self, IpcFault::PeerGone)
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Which deployment shape the channel is in. Selects whether the ordering
/// and duplicate clauses are enforceable at all: a local kernel queue cannot
/// reorder or duplicate, so claiming to have tested those clauses locally
/// would be a false coverage claim.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// Same host, kernel-mediated (Unix socket, pipe, POSIX mq, shared ring).
    /// The kernel guarantees in-order, exactly-once delivery.
    LocalQueue,
    /// Different hosts. Delivery may duplicate and reorder.
    CrossHost,
}

impl ChannelKind {
    /// Can the transport itself reorder or duplicate datagrams?
    pub const fn may_reorder(self) -> bool {
        matches!(self, ChannelKind::CrossHost)
    }
}

/// The channel's contractual parameters. Every timing term is an envelope,
/// never a point value — see the crate doc comment on why the envelopes here
/// are far wider than a wire bus's.
#[derive(Copy, Clone, Debug)]
pub struct IpcConfig {
    /// Deployment shape (gates the ordering/duplicate clauses).
    pub kind: ChannelKind,
    /// Send → receive latency envelope for one datagram.
    pub delivery: TimeBound,
    /// Receive → respond envelope: how long the responder may take to turn a
    /// request around. Under a non-real-time scheduler this is dominated by
    /// wakeup latency, not by the work itself.
    pub resp_time: TimeBound,
    /// Maximum datagram length in bytes (`IPC-MSG-MAXLEN`).
    pub max_msg_len: u32,
    /// Receive-queue depth in datagrams. A send into a full queue must be
    /// rejected with [`IpcFault::WouldBlock`].
    pub queue_depth: u16,
}

impl IpcConfig {
    /// A local Unix-domain-socket profile under a **non-real-time**
    /// scheduler: sub-microsecond delivery, but a response envelope spanning
    /// 10 µs (already runnable) to 20 ms (a full `SCHED_OTHER` timeslice
    /// behind a busy runqueue). That 2000× width is the whole point — it is
    /// the nondeterminism a single happy-path integration test never sees.
    pub const fn local_non_realtime() -> Self {
        IpcConfig {
            kind: ChannelKind::LocalQueue,
            delivery: TimeBound::new(Nanos::from_nanos(200), Nanos::from_micros(5)),
            resp_time: TimeBound::new(Nanos::from_micros(10), Nanos::from_millis(20)),
            max_msg_len: 4096,
            queue_depth: 8,
        }
    }

    /// A cross-host profile: delivery becomes a millisecond-scale envelope
    /// and the transport may duplicate/reorder, so `IPC-NO-DUPLICATE` and
    /// `IPC-ORDERING` become live obligations rather than kernel guarantees.
    pub const fn cross_host() -> Self {
        IpcConfig {
            kind: ChannelKind::CrossHost,
            delivery: TimeBound::new(Nanos::from_micros(50), Nanos::from_millis(5)),
            resp_time: TimeBound::new(Nanos::from_micros(10), Nanos::from_millis(20)),
            max_msg_len: 65536,
            queue_depth: 32,
        }
    }

    /// A real-time-scheduled local profile, for contrast: the response
    /// envelope narrows to something a wire bus would recognize. Present so
    /// the same contract can express "this deployment is actually tight" and
    /// have the oracle hold it to that.
    pub const fn local_realtime() -> Self {
        IpcConfig {
            kind: ChannelKind::LocalQueue,
            delivery: TimeBound::new(Nanos::from_nanos(200), Nanos::from_micros(2)),
            resp_time: TimeBound::new(Nanos::from_micros(5), Nanos::from_micros(200)),
            max_msg_len: 4096,
            queue_depth: 8,
        }
    }
}

// ---------------------------------------------------------------------------
// SW-facing HAL traits
// ---------------------------------------------------------------------------

/// A correlation id: ties a response to the request it answers.
pub type CorrelationId = u32;

/// Error returned by a send attempt.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IpcSendError {
    /// Queue full — retry later. Recoverable.
    WouldBlock,
    /// Peer is gone. Terminal.
    PeerGone,
    /// Datagram exceeds `max_msg_len`.
    TooLong,
}

/// A datagram as the software sees it: correlation id plus payload.
/// Fixed-capacity so the type stays `no_std` and alloc-free (G1/ADR-9's
/// reasoning, at the smaller size IPC control messages actually use).
pub const MAX_PAYLOAD: usize = 64;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub id: CorrelationId,
    pub buf: [u8; MAX_PAYLOAD],
    pub len: u8,
}

impl Datagram {
    pub fn new(id: CorrelationId, payload: &[u8]) -> Self {
        let mut buf = [0u8; MAX_PAYLOAD];
        let len = payload.len().min(MAX_PAYLOAD);
        buf[..len].copy_from_slice(&payload[..len]);
        Datagram {
            id,
            buf,
            len: len as u8,
        }
    }

    pub fn payload(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }
}

/// SW-facing requester port, poll-driven (ADR-2: no async, so the same
/// driver runs under the simulator and against a real socket).
pub trait IpcRequester {
    /// Attempt to send `msg`. `Err(WouldBlock)` when the queue is full —
    /// the caller must handle backpressure, not ignore it.
    fn try_send(&mut self, msg: &Datagram) -> Result<(), IpcSendError>;
    /// Non-blocking receive of a response, or a fault the environment
    /// injected.
    fn try_recv(&mut self) -> Option<Result<Datagram, IpcFault>>;
}

/// SW-facing responder, poll-driven. The reference implementation is
/// [`IpcResponderDevice`].
pub trait IpcResponder {
    /// A request arrived; return the response payload to send back, or
    /// `None` to deliberately stay silent (used by the bug catalog).
    fn on_request(&mut self, req: &Datagram) -> Option<Datagram>;
    /// The peer went away — release any per-connection state.
    fn on_peer_gone(&mut self);
}
