//! Deterministic IPC channel simulator: one requester actor, one responder
//! process, a bounded queue between them. Mirrors [`crate::spi::SpiWorld`]'s
//! determinism guarantees and structure but is kept independent (no merged
//! event enum, no shared `Sim` engine) per the same rule the other worlds
//! follow.
//!
//! # What this world samples that no other world does
//!
//! * **Scheduler wakeup latency.** Every response turnaround is drawn from
//!   `IpcConfig::resp_time`, an envelope that under `SCHED_OTHER` spans
//!   microseconds to tens of milliseconds. Where `SpiWorld` jitters a byte
//!   time by ±5%, this world draws across a range 2000× wide — so the seed
//!   space explores genuinely different orderings, not just slightly
//!   different timings.
//! * **Backpressure.** The queue is bounded (`queue_depth`); a send into a
//!   full queue is rejected, and the requester must cope. The rejection is
//!   emitted as a conforming `SendRejected` event, never a silent drop.
//! * **Peer death.** At a seeded probability the responder process exits
//!   mid-run. The world surfaces `PeerGone` *before* `PeerExit`, which is
//!   the behavior `IPC-PEER-DEATH` demands of a real implementation.
//!
//! Determinism: every draw comes from the scheduler's single seeded stream
//! in fixed event order, exactly as G2 requires.

use std::collections::VecDeque;
use std::fmt;

use stablehal_contract::{Nanos, Oracle, TraceEvent};
use stablehal_ipc::contract::{IpcContract, IpcEvent};
use stablehal_ipc::{Datagram, IpcConfig, IpcFault, IpcRequester, IpcResponder, IpcSendError};

use crate::sched::Scheduler;

/// What the requester actor wants after a poll — mirrors [`crate::NextPoll`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NextPoll {
    At(Nanos),
    Wait,
    Done,
}

/// Requester-side actor: polled like the UART/I2C/SPI actors; sees an
/// [`IpcRequester`] port.
pub trait IpcRequesterActor {
    fn poll(&mut self, now: Nanos, port: &mut dyn IpcRequester) -> NextPoll;

    /// What the requester recorded for each scripted step. Exposed on the
    /// actor trait (rather than only on the concrete adapter) so the world
    /// can surface it for *property* checks — the payload-correctness
    /// obligations that the channel contract deliberately does not judge.
    /// Defaults to empty for actors that record nothing.
    fn outcomes(&self) -> &[Option<stablehal_ipc::IpcStepOutcome>] {
        &[]
    }
}

/// Adapts a [`stablehal_ipc::ScriptedRequester`] into an
/// [`IpcRequesterActor`] for use inside [`IpcWorld`].
pub struct IpcRequesterAdapter {
    inner: stablehal_ipc::ScriptedRequester,
}

impl IpcRequesterAdapter {
    pub fn new(inner: stablehal_ipc::ScriptedRequester) -> Self {
        IpcRequesterAdapter { inner }
    }
    pub fn into_inner(self) -> stablehal_ipc::ScriptedRequester {
        self.inner
    }
    pub fn outcomes(&self) -> &[Option<stablehal_ipc::IpcStepOutcome>] {
        self.inner.outcomes()
    }
    pub fn rejections(&self) -> u32 {
        self.inner.rejections()
    }
}

impl IpcRequesterActor for IpcRequesterAdapter {
    fn poll(&mut self, now: Nanos, port: &mut dyn IpcRequester) -> NextPoll {
        let _ = now;
        // Drain every phase transition available this tick — same fix as
        // `I2cMasterAdapter::poll` / `SpiMasterAdapter::poll`.
        while self.inner.step(port) {}
        if self.inner.done() {
            NextPoll::Done
        } else {
            NextPoll::Wait
        }
    }

    fn outcomes(&self) -> &[Option<stablehal_ipc::IpcStepOutcome>] {
        self.inner.outcomes()
    }
}

#[derive(Debug)]
pub enum IpcSimError {
    Contract { violation: String },
}

impl fmt::Display for IpcSimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcSimError::Contract { violation } => write!(f, "IPC contract violated: {violation}"),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct IpcRunSummary {
    pub events: usize,
    pub requester_done: bool,
    pub ended_at: Nanos,
    /// How many sends the bounded queue rejected (backpressure exercised).
    pub rejections: u32,
    /// Whether the responder process died during this run.
    pub peer_died: bool,
}

pub struct IpcWorldConfig {
    pub seed: u64,
    pub config: IpcConfig,
    /// Probability (in parts per million, per request) that the responder
    /// process exits mid-transaction. 0 disables peer-death injection.
    pub peer_death_ppm: u32,
    /// Extra queue occupancy to simulate a competing producer, in
    /// datagrams. Raising this toward `queue_depth` makes backpressure
    /// common; the default 0 leaves the queue to the requester alone.
    pub background_load: u16,
}

impl IpcWorldConfig {
    pub fn new(seed: u64, config: IpcConfig) -> Self {
        IpcWorldConfig {
            seed,
            config,
            peer_death_ppm: 0,
            background_load: 0,
        }
    }

    /// Enable seeded peer-death injection at `ppm` per request.
    pub fn with_peer_death(mut self, ppm: u32) -> Self {
        self.peer_death_ppm = ppm;
        self
    }

    /// Pre-occupy `n` queue slots to force backpressure.
    pub fn with_background_load(mut self, n: u16) -> Self {
        self.background_load = n;
        self
    }
}

enum Job {
    PollRequester,
    /// A request datagram arrives at the responder.
    DeliverRequest(Datagram),
    /// The responder's scheduler slice finally runs; it answers now.
    Respond(Datagram),
    /// A response datagram arrives back at the requester.
    DeliverResponse(Datagram),
    /// The responder process exits.
    PeerExit,
}

/// The bounded queue plus the requester's inbox, as the requester's port
/// sees them.
#[derive(Default)]
struct RequesterPort {
    /// Datagrams the requester handed off this tick, awaiting the engine.
    outbox: Option<Datagram>,
    /// Responses (or faults) ready for the requester to collect.
    inbox: VecDeque<Result<Datagram, IpcFault>>,
    /// Current occupancy of the responder's receive queue.
    queued: u16,
    /// Capacity of that queue.
    depth: u16,
    /// Max datagram length.
    max_len: u32,
    /// Set once the peer is gone: every later send fails terminally.
    peer_gone: bool,
    /// Rejections this run, for the summary.
    rejections: u32,
    /// Set when a send was rejected, so the engine can emit the conforming
    /// `SendRejected` event for it.
    rejected_id: Option<u32>,
}

impl IpcRequester for RequesterPort {
    fn try_send(&mut self, msg: &Datagram) -> Result<(), IpcSendError> {
        if self.peer_gone {
            return Err(IpcSendError::PeerGone);
        }
        if msg.len as u32 > self.max_len {
            return Err(IpcSendError::TooLong);
        }
        if self.queued >= self.depth {
            // Bounded queue full: reject loudly. This is the conforming
            // backpressure path (`IPC-BACKPRESSURE`).
            self.rejections += 1;
            self.rejected_id = Some(msg.id);
            return Err(IpcSendError::WouldBlock);
        }
        if self.outbox.is_some() {
            // One in-flight handoff per tick; the engine drains it.
            return Err(IpcSendError::WouldBlock);
        }
        self.queued += 1;
        self.outbox = Some(*msg);
        Ok(())
    }

    fn try_recv(&mut self) -> Option<Result<Datagram, IpcFault>> {
        self.inbox.pop_front()
    }
}

pub struct IpcWorld {
    sched: Scheduler<Job>,
    requester_actor: Box<dyn IpcRequesterActor>,
    port: RequesterPort,
    responder: Box<dyn IpcResponder>,
    config: IpcConfig,
    peer_death_ppm: u32,
    oracle: Oracle<IpcContract>,
    trace: Vec<TraceEvent<IpcEvent>>,
    requester_done: bool,
    peer_died: bool,
    in_flight: usize,
    ended: bool,
}

impl IpcWorld {
    pub fn new(
        cfg: IpcWorldConfig,
        requester: Box<dyn IpcRequesterActor>,
        responder: Box<dyn IpcResponder>,
    ) -> Self {
        let depth = cfg.config.queue_depth;
        IpcWorld {
            sched: Scheduler::new(cfg.seed),
            requester_actor: requester,
            port: RequesterPort {
                queued: cfg.background_load.min(depth),
                depth,
                max_len: cfg.config.max_msg_len,
                ..RequesterPort::default()
            },
            responder,
            config: cfg.config,
            peer_death_ppm: cfg.peer_death_ppm,
            oracle: Oracle::new(IpcContract::new(cfg.config)),
            trace: Vec::new(),
            requester_done: false,
            peer_died: false,
            in_flight: 0,
            ended: false,
        }
    }

    pub fn trace(&self) -> &[TraceEvent<IpcEvent>] {
        &self.trace
    }
    pub fn trace_hash(&self) -> u64 {
        stablehal_contract::trace::trace_hash(self.trace.iter())
    }
    pub fn now(&self) -> Nanos {
        self.sched.now()
    }
    pub fn rejections(&self) -> u32 {
        self.port.rejections
    }

    /// What the requester recorded per scripted step — the hook for
    /// *property* checks (payload correctness), which the channel contract
    /// deliberately does not judge. See `ipc_dst.rs`'s
    /// `wrong_payload_bug_*` pair.
    pub fn requester_outcomes(&self) -> &[Option<stablehal_ipc::IpcStepOutcome>] {
        self.requester_actor.outcomes()
    }

    fn emit(&mut self, event: IpcEvent) -> Result<(), IpcSimError> {
        let now = self.sched.now();
        self.trace.push(TraceEvent { at: now, event });
        self.oracle
            .observe(now, &event)
            .map_err(|v| IpcSimError::Contract {
                violation: format!("{v}"),
            })
    }

    /// Draw a delivery latency from the transport envelope.
    fn draw_delivery(&mut self) -> Nanos {
        let b = self.config.delivery;
        self.sched.prng().pick(b)
    }

    /// Draw a responder turnaround from the scheduling envelope. This is the
    /// draw that matters: on a non-real-time host the envelope is enormous,
    /// so distinct seeds produce qualitatively different runs (fast path vs.
    /// a stall that nearly times the transaction out), not merely jittered
    /// ones.
    fn draw_resp_time(&mut self) -> Nanos {
        let b = self.config.resp_time;
        self.sched.prng().pick(b)
    }

    fn poll_requester(&mut self) -> Result<(), IpcSimError> {
        if self.requester_done || self.peer_died {
            return Ok(());
        }
        let now = self.sched.now();
        let next = self.requester_actor.poll(now, &mut self.port);

        // A rejected send is a conforming, observable event — emit it so the
        // oracle can confirm backpressure was surfaced rather than dropped.
        if let Some(id) = self.port.rejected_id.take() {
            self.emit(IpcEvent::SendRejected { id })?;
        }

        if let Some(msg) = self.port.outbox.take() {
            self.emit(IpcEvent::Sent {
                id: msg.id,
                len: msg.len as u32,
            })?;
            self.in_flight += 1;
            let at = self.sched.now() + self.draw_delivery();
            self.sched.schedule(at, Job::DeliverRequest(msg));
        }

        match next {
            NextPoll::At(t) => self.sched.schedule(t, Job::PollRequester),
            NextPoll::Wait => {}
            NextPoll::Done => self.requester_done = true,
        }
        Ok(())
    }

    fn deliver_request(&mut self, msg: Datagram) -> Result<(), IpcSimError> {
        self.emit(IpcEvent::Delivered { id: msg.id })?;
        // The datagram has left the queue and is now in the responder's
        // hands, so a queue slot frees up.
        self.port.queued = self.port.queued.saturating_sub(1);

        // Seeded peer death: drawn once per delivered request, in fixed
        // event order.
        if self.peer_death_ppm > 0
            && self
                .sched
                .prng()
                .chance(self.peer_death_ppm as u64, 1_000_000)
        {
            // Surface the fault first, then exit — the ordering
            // `IPC-PEER-DEATH` requires of a conforming implementation.
            self.emit(IpcEvent::Fault {
                fault: IpcFault::PeerGone,
            })?;
            self.port.peer_gone = true;
            self.port.inbox.push_back(Err(IpcFault::PeerGone));
            self.responder.on_peer_gone();
            let now = self.sched.now();
            self.sched.schedule(now, Job::PeerExit);
            self.sched.schedule(now, Job::PollRequester);
            return Ok(());
        }

        let at = self.sched.now() + self.draw_resp_time();
        self.sched.schedule(at, Job::Respond(msg));
        Ok(())
    }

    fn respond(&mut self, req: Datagram) -> Result<(), IpcSimError> {
        match self.responder.on_request(&req) {
            Some(resp) => {
                self.emit(IpcEvent::Responded {
                    id: resp.id,
                    len: resp.len as u32,
                })?;
                let at = self.sched.now() + self.draw_delivery();
                self.sched.schedule(at, Job::DeliverResponse(resp));
            }
            None => {
                // The responder stayed silent. Nothing is emitted here —
                // the omission is caught at EndOfObservation as an
                // IPC-RESP-TIME breach, which is exactly the "silent hang"
                // failure this contract is meant to make loud.
                self.in_flight = self.in_flight.saturating_sub(1);
            }
        }
        Ok(())
    }

    fn deliver_response(&mut self, resp: Datagram) -> Result<(), IpcSimError> {
        self.emit(IpcEvent::ResponseDelivered { id: resp.id })?;
        self.in_flight = self.in_flight.saturating_sub(1);
        self.port.inbox.push_back(Ok(resp));
        let now = self.sched.now();
        self.sched.schedule(now, Job::PollRequester);
        Ok(())
    }

    pub fn run_until(&mut self, deadline: Nanos) -> Result<IpcRunSummary, IpcSimError> {
        let start = self.sched.now();
        self.sched.schedule(start, Job::PollRequester);

        while let Some((_now, job)) = self.sched.pop_due(deadline) {
            match job {
                Job::PollRequester => self.poll_requester()?,
                Job::DeliverRequest(m) => self.deliver_request(m)?,
                Job::Respond(m) => self.respond(m)?,
                Job::DeliverResponse(m) => self.deliver_response(m)?,
                Job::PeerExit => {
                    self.emit(IpcEvent::PeerExit)?;
                    self.peer_died = true;
                    break;
                }
            }
            if self.requester_done && self.in_flight == 0 && self.port.inbox.is_empty() {
                break;
            }
        }

        if !self.ended {
            self.emit(IpcEvent::EndOfObservation)?;
            self.ended = true;
        }

        Ok(IpcRunSummary {
            events: self.trace.len(),
            requester_done: self.requester_done,
            ended_at: self.sched.now(),
            rejections: self.port.rejections,
            peer_died: self.peer_died,
        })
    }
}
