//! Deterministic LIN bus simulator: one master actor, N slave devices,
//! sharing a `Scheduler`. Mirrors [`crate::Sim`]'s determinism guarantees
//! (single seeded PRNG stream, `(at, seq)` total event order) but is kept
//! independent per WP2 §2.3 (same rationale as [`crate::i2c::I2cWorld`]) —
//! no merged event enum, no shared `Sim` engine.
//!
//! Engine mechanics: each scheduled slot runs as one unit — emit the header
//! (break/delimiter/sync/PID with jittered timing), then, if a device owns
//! the frame ID, emit its response bytes + checksum with jittered
//! inter-byte gaps; otherwise emit `NoResponse`. Unlike I2C's byte-at-a-time
//! addressing, LIN has no per-byte ACK handshake, so one slot advances in a
//! single `advance()` call rather than a multi-step state machine.

use std::fmt;

use stablehal_contract::{Nanos, Oracle, TraceEvent};
use stablehal_lin::contract::{encode_pid, enhanced_checksum, LinContract, LinEvent};
use stablehal_lin::timing::LinTiming;
use stablehal_lin::{LinBus, LinBusy, LinSlaveDevice, ScheduleMaster, SlotOutcome};

use crate::sched::Scheduler;

/// What the master actor wants after a poll — mirrors [`crate::NextPoll`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NextPoll {
    At(Nanos),
    Wait,
    Done,
}

/// Master-side actor: polled like UART/I2C actors; sees a [`LinBus`] port.
pub trait LinMasterActor {
    fn poll(&mut self, now: Nanos, port: &mut dyn LinBus) -> NextPoll;
}

/// Adapts a [`ScheduleMaster`] into a [`LinMasterActor`] for use inside
/// [`LinWorld`].
pub struct LinMasterAdapter {
    inner: ScheduleMaster,
}

impl LinMasterAdapter {
    pub fn new(inner: ScheduleMaster) -> Self {
        LinMasterAdapter { inner }
    }
    pub fn into_inner(self) -> ScheduleMaster {
        self.inner
    }
    pub fn outcomes(&self) -> &[Option<SlotOutcome>] {
        self.inner.outcomes()
    }
}

impl LinMasterActor for LinMasterAdapter {
    fn poll(&mut self, now: Nanos, port: &mut dyn LinBus) -> NextPoll {
        let _ = now;
        // `ScheduleMaster::step` advances exactly one phase transition per
        // call (Idle -> InFlight sends a header; InFlight -> Idle collects a
        // finished slot's outcome). Drain every transition available *this*
        // tick — otherwise a just-collected slot's Idle->InFlight header for
        // the *next* slot never gets sent until some later event wakes us,
        // which never arrives once the current slot has fully closed out,
        // silently stalling the schedule after its first slot (mirrors the
        // same fix in `I2cMasterAdapter::poll`).
        while self.inner.step(port) {}
        if self.inner.done() {
            NextPoll::Done
        } else {
            NextPoll::Wait
        }
    }
}

#[derive(Debug)]
pub enum LinSimError {
    Contract { violation: String },
}

impl fmt::Display for LinSimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinSimError::Contract { violation } => write!(f, "LIN contract violated: {violation}"),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct LinRunSummary {
    pub events: usize,
    pub master_done: bool,
    pub ended_at: Nanos,
}

pub struct LinWorldConfig {
    pub seed: u64,
    pub timing: LinTiming,
    /// Per-byte-time jitter as a fraction of the nominal, in parts per
    /// 1000 (e.g. 10 = +/-1%). Drawn once per header/byte from the seeded
    /// stream — same discipline as `I2cWorldConfig::jitter_permille`.
    pub jitter_permille: u32,
}

impl LinWorldConfig {
    pub fn new(seed: u64, timing: LinTiming) -> Self {
        LinWorldConfig {
            seed,
            timing,
            jitter_permille: 10,
        }
    }
}

#[derive(Default)]
struct MasterPort {
    pending_pid: Option<u8>,
    busy: bool,
    result: Option<SlotOutcome>,
}

impl LinBus for MasterPort {
    fn try_send_header(&mut self, pid: u8) -> Result<(), LinBusy> {
        if self.busy {
            return Err(LinBusy);
        }
        self.pending_pid = Some(pid);
        self.busy = true;
        Ok(())
    }

    fn poll_response(&mut self, buf: &mut [u8]) -> Option<SlotOutcome> {
        let outcome = self.result.take()?;
        if let SlotOutcome::Responded { len } = outcome {
            let _ = len;
            let _ = buf; // engine writes directly into the trace; the reference
                         // master in this crate does not need response bytes
                         // for its own logic (schedule-driven, not data-driven).
        }
        self.busy = false;
        Some(outcome)
    }
}

enum Job {
    PollMaster,
    /// Fire the header for a just-accepted `try_send_header`, or the
    /// response phase after the header completes.
    Advance,
}

pub struct LinWorld {
    sched: Scheduler<Job>,
    master_actor: Box<dyn LinMasterActor>,
    master_port: MasterPort,
    /// (pid, id) of the header currently awaiting a response phase.
    slot: Option<(u8, u8)>,
    slaves: Vec<Box<dyn LinSlaveDevice>>,
    timing: LinTiming,
    jitter_permille: u32,
    oracle: Oracle<LinContract>,
    trace: Vec<TraceEvent<LinEvent>>,
    master_done: bool,
    ended: bool,
    publishers: Vec<(u8, u8)>,
}

impl LinWorld {
    pub fn new(config: LinWorldConfig, master: Box<dyn LinMasterActor>) -> Self {
        LinWorld {
            sched: Scheduler::new(config.seed),
            master_actor: master,
            master_port: MasterPort::default(),
            slot: None,
            slaves: Vec::new(),
            timing: config.timing,
            jitter_permille: config.jitter_permille,
            oracle: Oracle::new(LinContract::new(config.timing, &[])),
            trace: Vec::new(),
            master_done: false,
            ended: false,
            publishers: Vec::new(),
        }
    }

    /// Register a slave and the (id, n_data) pairs it publishes, so the
    /// contract can judge `LIN-SLAVE-SILENCE`/frame sizing correctly.
    pub fn add_slave(&mut self, dev: Box<dyn LinSlaveDevice>, published: &[(u8, u8)]) {
        self.slaves.push(dev);
        self.publishers.extend_from_slice(published);
        self.oracle = Oracle::new(LinContract::new(self.timing, &self.publishers));
    }

    pub fn trace(&self) -> &[TraceEvent<LinEvent>] {
        &self.trace
    }
    pub fn trace_hash(&self) -> u64 {
        stablehal_contract::trace::trace_hash(self.trace.iter())
    }
    pub fn now(&self) -> Nanos {
        self.sched.now()
    }

    fn emit(&mut self, event: LinEvent) -> Result<(), LinSimError> {
        let now = self.sched.now();
        self.trace.push(TraceEvent { at: now, event });
        self.oracle
            .observe(now, &event)
            .map_err(|v| LinSimError::Contract {
                violation: format!("{v}"),
            })
    }

    /// Jittered duration around `nominal`, drawn once from the seeded
    /// stream — mirrors `I2cWorld::draw_byte_time`. Suitable for envelopes
    /// with a real maximum (byte/inter-byte gaps); never use this for a
    /// hard minimum-only obligation (break, break delimiter) — a downward
    /// jitter would make the reference master breach its own contract.
    fn jitter(&mut self, nominal: Nanos) -> Nanos {
        let nominal_ns = nominal.as_nanos() as i64;
        let jitter = self.jitter_permille as i64;
        let roll = if jitter == 0 {
            0
        } else {
            self.sched.prng().range_inclusive(0, 2 * jitter as u64) as i64 - jitter
        };
        let delta = nominal_ns * roll / 1000;
        Nanos::from_nanos((nominal_ns + delta).max(1) as u64)
    }

    /// Jittered duration that never drops below `min` — for obligations
    /// that only have a floor (break field, break delimiter): the
    /// reference master must always emit a legal value, so jitter only
    /// ever adds margin above `min`, drawn from `[0, jitter_permille]`
    /// parts-per-1000 of `min` itself.
    fn jitter_at_least(&mut self, min: Nanos) -> Nanos {
        let min_ns = min.as_nanos() as i64;
        let jitter = self.jitter_permille as i64;
        let extra = if jitter == 0 {
            0
        } else {
            self.sched.prng().range_inclusive(0, jitter as u64) as i64
        };
        let delta = min_ns * extra / 1000;
        Nanos::from_nanos((min_ns + delta) as u64)
    }

    fn poll_master(&mut self) -> Result<(), LinSimError> {
        if self.master_done {
            return Ok(());
        }
        let now = self.sched.now();
        let next = self.master_actor.poll(now, &mut self.master_port);

        if self.slot.is_none() {
            if let Some(pid) = self.master_port.pending_pid.take() {
                self.begin_header(pid)?;
            }
        }

        match next {
            NextPoll::At(t) => self.sched.schedule(t, Job::PollMaster),
            NextPoll::Wait => {}
            NextPoll::Done => self.master_done = true,
        }
        Ok(())
    }

    fn begin_header(&mut self, pid: u8) -> Result<(), LinSimError> {
        let id = pid & 0x3F;
        let break_dur = self.jitter_at_least(self.timing.break_min());
        self.emit(LinEvent::Break { dur: break_dur })?;
        let delim_dur = self.jitter_at_least(self.timing.break_delimiter_min());
        self.emit(LinEvent::BreakDelimiter { dur: delim_dur })?;
        self.emit(LinEvent::Sync { byte: 0x55 })?;
        let parity_ok = encode_pid(id) == pid;
        self.emit(LinEvent::Pid { id, parity_ok })?;
        self.slot = Some((pid, id));
        let dur = self.jitter(self.timing.byte_time());
        let at = self.sched.now() + dur;
        self.sched.schedule(at, Job::Advance);
        Ok(())
    }

    fn advance(&mut self) -> Result<(), LinSimError> {
        let (pid, id) = match self.slot.take() {
            Some(p) => p,
            None => return Ok(()),
        };
        let mut response: Option<([u8; 8], u8, u8)> = None;
        for slave in self.slaves.iter_mut() {
            if let Some(r) = slave.respond(pid, id) {
                response = Some(r);
                break;
            }
        }
        match response {
            None => {
                self.emit(LinEvent::NoResponse)?;
                self.master_port.result = Some(SlotOutcome::NoResponse);
            }
            Some((data, len, checksum)) => {
                let mut gap = self.jitter(self.timing.byte_time());
                for &byte in &data[..len as usize] {
                    self.emit(LinEvent::DataByte { byte, gap })?;
                    gap = self.jitter(self.timing.byte_time());
                }
                let expected = enhanced_checksum(pid, &data[..len as usize]);
                self.emit(LinEvent::Checksum {
                    byte: checksum,
                    expected,
                    gap,
                })?;
                self.master_port.result = Some(SlotOutcome::Responded { len });
            }
        }
        let now = self.sched.now();
        self.sched.schedule(now, Job::PollMaster);
        Ok(())
    }

    pub fn run_until(&mut self, deadline: Nanos) -> Result<LinRunSummary, LinSimError> {
        let start = self.sched.now();
        self.sched.schedule(start, Job::PollMaster);

        while let Some((_now, job)) = self.sched.pop_due(deadline) {
            match job {
                Job::PollMaster => self.poll_master()?,
                Job::Advance => self.advance()?,
            }
            if self.master_done && self.slot.is_none() && self.master_port.result.is_none() {
                break;
            }
        }

        if !self.ended {
            self.emit(LinEvent::EndOfObservation)?;
            self.ended = true;
        }

        Ok(LinRunSummary {
            events: self.trace.len(),
            master_done: self.master_done,
            ended_at: self.sched.now(),
        })
    }
}
