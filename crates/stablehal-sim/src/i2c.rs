//! Deterministic I2C bus simulator: one master actor, N slave devices,
//! sharing a [`Scheduler`]. Mirrors [`crate::Sim`]'s determinism guarantees
//! (single seeded PRNG stream, `(at, seq)` total event order) but is kept
//! independent per WP2 §2.3 — no merged event enum, no shared `Sim` engine.
//!
//! Engine mechanics (WP2 §2.2.4): each in-flight op executes byte by byte on
//! the scheduler. Per byte the engine (a) draws jitter for that byte's
//! duration, (b) asks the addressed slave for ack/stretch, (c) emits the
//! wire events through the oracle, (d) schedules the next byte (or op
//! completion) at `now + byte_time (+ stretch)`. A Start is deferred until
//! `bus_free_min` has elapsed since the previous Stop.

use std::fmt;

use stablehal_contract::{Nanos, Oracle, TraceEvent};
use stablehal_i2c::contract::{I2cContract, I2cEvent};
use stablehal_i2c::timing::I2cTiming;
use stablehal_i2c::{I2cBusy, I2cFault, I2cMaster, I2cOp, I2cSlaveDevice, ScriptedMaster};

use crate::sched::Scheduler;

/// Max bytes per single write/read op the sim engine buffers internally.
const MAX_OP_BYTES: usize = 32;

/// What the master actor wants after a poll — mirrors [`crate::NextPoll`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NextPoll {
    At(Nanos),
    /// No new poll scheduled by a timer; the engine will re-poll when the
    /// in-flight op resolves.
    Wait,
    Done,
}

/// Master-side actor: polled like UART actors; sees an [`I2cMaster`] port.
pub trait I2cMasterActor {
    fn poll(&mut self, now: Nanos, port: &mut dyn I2cMaster) -> NextPoll;
}

/// Adapts a [`ScriptedMaster`] into an [`I2cMasterActor`] for use inside
/// [`I2cWorld`].
pub struct I2cMasterAdapter {
    inner: ScriptedMaster,
}

impl I2cMasterAdapter {
    pub fn new(inner: ScriptedMaster) -> Self {
        I2cMasterAdapter { inner }
    }
    pub fn into_inner(self) -> ScriptedMaster {
        self.inner
    }
    pub fn outcomes(&self) -> &[Option<stablehal_i2c::StepOutcome>] {
        self.inner.outcomes()
    }
}

impl I2cMasterActor for I2cMasterAdapter {
    fn poll(&mut self, now: Nanos, port: &mut dyn I2cMaster) -> NextPoll {
        let _ = now;
        self.inner.step(port);
        if self.inner.done() {
            NextPoll::Done
        } else {
            // The engine wakes the master again exactly when the in-flight
            // op resolves (finish_op*/PollMaster) or a deferred Start opens
            // — never on a timer — so no `At(t)` re-poll is needed here.
            NextPoll::Wait
        }
    }
}

#[derive(Debug)]
pub enum I2cSimError {
    /// The bus contract itself was violated.
    Contract { violation: String },
}

impl fmt::Display for I2cSimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            I2cSimError::Contract { violation } => write!(f, "I2C contract violated: {violation}"),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct I2cRunSummary {
    pub events: usize,
    pub master_done: bool,
    pub ended_at: Nanos,
}

pub struct I2cWorldConfig {
    pub seed: u64,
    pub timing: I2cTiming,
    /// Per-byte SCL jitter as a fraction of the period, in parts per 1000
    /// (e.g. 10 = ±1%). Drawn once per byte from the seeded stream.
    pub jitter_permille: u32,
}

impl I2cWorldConfig {
    pub fn new(seed: u64, timing: I2cTiming) -> Self {
        I2cWorldConfig {
            seed,
            timing,
            jitter_permille: 10,
        }
    }
}

enum PendingOp {
    Write {
        addr7: u8,
        len: u8,
        buf: [u8; MAX_OP_BYTES],
        stop: bool,
    },
    Read {
        addr7: u8,
        len: u8,
        stop: bool,
    },
}

/// One in-flight master operation, decomposed into a byte-at-a-time plan the
/// engine steps through on the scheduler.
enum InFlight {
    /// Address byte not yet emitted.
    Addressing { op: PendingOp },
    Write {
        addr7: u8,
        buf: [u8; MAX_OP_BYTES],
        len: u8,
        idx: u8,
        stop: bool,
    },
    Read {
        addr7: u8,
        len: u8,
        idx: u8,
        buf: [u8; MAX_OP_BYTES],
        stop: bool,
    },
}

enum Job {
    PollMaster,
    /// Fire the next byte-phase transition of the in-flight op, or (when
    /// nothing is in flight yet) attempt to open a deferred Start once
    /// `bus_free_min` has elapsed.
    Advance,
}

#[derive(Default)]
struct MasterPort {
    pending: Option<PendingOp>,
    busy: bool,
    result: Option<(Result<(), I2cFault>, [u8; MAX_OP_BYTES], u8)>,
}

impl I2cMaster for MasterPort {
    fn try_begin(&mut self, op: &I2cOp<'_>) -> Result<(), I2cBusy> {
        if self.busy {
            return Err(I2cBusy);
        }
        let pending = match *op {
            I2cOp::Write { addr7, bytes, stop } => {
                let mut buf = [0u8; MAX_OP_BYTES];
                let len = bytes.len().min(MAX_OP_BYTES);
                buf[..len].copy_from_slice(&bytes[..len]);
                PendingOp::Write {
                    addr7,
                    len: len as u8,
                    buf,
                    stop,
                }
            }
            I2cOp::Read { addr7, len, stop } => PendingOp::Read {
                addr7,
                len: len.min(MAX_OP_BYTES as u8),
                stop,
            },
        };
        self.pending = Some(pending);
        self.busy = true;
        Ok(())
    }

    fn poll_done(&mut self, buf: &mut [u8]) -> Option<Result<(), I2cFault>> {
        let (result, data, len) = self.result.take()?;
        let n = len as usize;
        buf[..n].copy_from_slice(&data[..n]);
        self.busy = false;
        Some(result)
    }
}

pub struct I2cWorld {
    sched: Scheduler<Job>,
    master_actor: Box<dyn I2cMasterActor>,
    master_port: MasterPort,
    in_flight: Option<InFlight>,
    /// An op waiting for `bus_free_min` to elapse before its Start.
    deferred_start: Option<PendingOp>,
    slaves: Vec<Box<dyn I2cSlaveDevice>>,
    timing: I2cTiming,
    jitter_permille: u32,
    oracle: Oracle<I2cContract>,
    trace: Vec<TraceEvent<I2cEvent>>,
    master_done: bool,
    bus_open: bool,
    /// Virtual time of the most recent Stop, if any — the earliest a new
    /// Start may legally begin is this plus `bus_free_min`.
    last_stop_at: Option<Nanos>,
    ended: bool,
}

impl I2cWorld {
    pub fn new(config: I2cWorldConfig, master: Box<dyn I2cMasterActor>) -> Self {
        I2cWorld {
            sched: Scheduler::new(config.seed),
            master_actor: master,
            master_port: MasterPort::default(),
            in_flight: None,
            deferred_start: None,
            slaves: Vec::new(),
            timing: config.timing,
            jitter_permille: config.jitter_permille,
            oracle: Oracle::new(I2cContract::new(config.timing, &[])),
            trace: Vec::new(),
            master_done: false,
            bus_open: false,
            last_stop_at: None,
            ended: false,
        }
    }

    pub fn add_slave(&mut self, dev: Box<dyn I2cSlaveDevice>) {
        self.slaves.push(dev);
        let addrs: Vec<u8> = self.slaves.iter().map(|s| s.addr7()).collect();
        self.oracle = Oracle::new(I2cContract::new(self.timing, &addrs));
    }

    pub fn trace(&self) -> &[TraceEvent<I2cEvent>] {
        &self.trace
    }
    pub fn trace_hash(&self) -> u64 {
        stablehal_contract::trace::trace_hash(self.trace.iter())
    }
    pub fn now(&self) -> Nanos {
        self.sched.now()
    }

    fn emit(&mut self, event: I2cEvent) -> Result<(), I2cSimError> {
        let now = self.sched.now();
        self.trace.push(TraceEvent { at: now, event });
        self.oracle
            .observe(now, &event)
            .map_err(|v| I2cSimError::Contract {
                violation: format!("{v}"),
            })
    }

    fn slave_for(&self, addr7: u8) -> Option<usize> {
        self.slaves.iter().position(|s| s.addr7() == addr7)
    }

    /// One byte-time tick, jittered from the seeded stream (drawn in fixed
    /// event order — one draw per byte — mirroring `Sim::run_until`'s
    /// draw-in-event-order discipline).
    fn draw_byte_time(&mut self) -> Nanos {
        let nominal = I2cTiming::byte_time(self.timing.scl.max).as_nanos() as i64;
        let jitter = self.jitter_permille as i64;
        let roll = if jitter == 0 {
            0
        } else {
            self.sched.prng().range_inclusive(0, 2 * jitter as u64) as i64 - jitter
        };
        let delta = nominal * roll / 1000;
        Nanos::from_nanos((nominal + delta).max(1) as u64)
    }

    fn poll_master(&mut self) -> Result<(), I2cSimError> {
        if self.master_done {
            return Ok(());
        }
        let now = self.sched.now();
        let next = self.master_actor.poll(now, &mut self.master_port);

        if self.in_flight.is_none() && self.deferred_start.is_none() {
            if let Some(pending) = self.master_port.pending.take() {
                self.request_start(pending)?;
            }
        }

        match next {
            NextPoll::At(t) => self.sched.schedule(t, Job::PollMaster),
            NextPoll::Wait => {}
            NextPoll::Done => self.master_done = true,
        }
        Ok(())
    }

    /// Either open the transaction immediately, or defer it until
    /// `bus_free_min` has elapsed since the last Stop.
    fn request_start(&mut self, op: PendingOp) -> Result<(), I2cSimError> {
        let now = self.sched.now();
        let ready_at = self
            .last_stop_at
            .map(|t| t + self.timing.bus_free_min)
            .unwrap_or(now);
        if now >= ready_at {
            self.open_start(op)?;
        } else {
            self.deferred_start = Some(op);
            self.sched.schedule(ready_at, Job::Advance);
        }
        Ok(())
    }

    fn open_start(&mut self, op: PendingOp) -> Result<(), I2cSimError> {
        self.emit(I2cEvent::Start)?;
        self.bus_open = true;
        self.in_flight = Some(InFlight::Addressing { op });
        let dur = self.draw_byte_time();
        let at = self.sched.now() + dur;
        self.sched.schedule(at, Job::Advance);
        Ok(())
    }

    fn addr_phase(&mut self, addr7: u8, read: bool) -> Result<bool, I2cSimError> {
        let idx = self.slave_for(addr7);
        let acked = idx.is_some_and(|i| self.slaves[i].ack_addr());
        self.emit(I2cEvent::AddrByte { addr7, read, acked })?;
        if let Some(i) = idx {
            self.slaves[i].on_start();
        }
        Ok(acked)
    }

    fn advance(&mut self) -> Result<(), I2cSimError> {
        // A deferred Start becoming eligible.
        if self.in_flight.is_none() {
            if let Some(op) = self.deferred_start.take() {
                let now = self.sched.now();
                let ready_at = self
                    .last_stop_at
                    .map(|t| t + self.timing.bus_free_min)
                    .unwrap_or(now);
                if now >= ready_at {
                    self.open_start(op)?;
                } else {
                    self.deferred_start = Some(op);
                    self.sched.schedule(ready_at, Job::Advance);
                }
            }
            return Ok(());
        }

        let inflight = self.in_flight.take().unwrap();
        match inflight {
            InFlight::Addressing { op } => {
                let (addr7, read, stop_flag) = match &op {
                    PendingOp::Write { addr7, stop, .. } => (*addr7, false, *stop),
                    PendingOp::Read { addr7, stop, .. } => (*addr7, true, *stop),
                };
                let acked = self.addr_phase(addr7, read)?;
                if !acked {
                    self.finish_op(Err(I2cFault::AddrNack))?;
                    return Ok(());
                }
                self.in_flight = Some(match op {
                    PendingOp::Write {
                        addr7,
                        len,
                        buf,
                        stop,
                    } => InFlight::Write {
                        addr7,
                        buf,
                        len,
                        idx: 0,
                        stop,
                    },
                    PendingOp::Read { addr7, len, stop } => InFlight::Read {
                        addr7,
                        len,
                        idx: 0,
                        buf: [0u8; MAX_OP_BYTES],
                        stop,
                    },
                });
                let _ = stop_flag;
                let dur = self.draw_byte_time();
                let at = self.sched.now() + dur;
                self.sched.schedule(at, Job::Advance);
            }
            InFlight::Write {
                addr7,
                buf,
                len,
                idx,
                stop,
            } => {
                if idx < len {
                    let byte = buf[idx as usize];
                    let slave_idx = self.slave_for(addr7);
                    let (acked, stretch) = if let Some(i) = slave_idx {
                        self.slaves[i].on_write(byte)
                    } else {
                        (false, Nanos::ZERO)
                    };
                    self.emit(I2cEvent::DataByte {
                        master_to_slave: true,
                        byte,
                        acked,
                    })?;
                    if stretch > Nanos::ZERO {
                        self.emit(I2cEvent::Stretch { dur: stretch })?;
                    }
                    if !acked {
                        self.finish_op(Err(I2cFault::DataNack { at_byte: idx }))?;
                        return Ok(());
                    }
                    self.in_flight = Some(InFlight::Write {
                        addr7,
                        buf,
                        len,
                        idx: idx + 1,
                        stop,
                    });
                    let dur = self.draw_byte_time() + stretch;
                    let at = self.sched.now() + dur;
                    self.sched.schedule(at, Job::Advance);
                } else {
                    self.finish_op_completed(stop, [0u8; MAX_OP_BYTES], 0)?;
                }
            }
            InFlight::Read {
                addr7,
                len,
                idx,
                mut buf,
                stop,
            } => {
                if idx < len {
                    let slave_idx = self.slave_for(addr7);
                    let (byte, stretch) = if let Some(i) = slave_idx {
                        self.slaves[i].on_read()
                    } else {
                        (0xFF, Nanos::ZERO)
                    };
                    let is_last = idx + 1 == len;
                    let master_acks = !is_last;
                    buf[idx as usize] = byte;
                    self.emit(I2cEvent::DataByte {
                        master_to_slave: false,
                        byte,
                        acked: master_acks,
                    })?;
                    if stretch > Nanos::ZERO {
                        self.emit(I2cEvent::Stretch { dur: stretch })?;
                    }
                    self.in_flight = Some(InFlight::Read {
                        addr7,
                        len,
                        idx: idx + 1,
                        buf,
                        stop,
                    });
                    let dur = self.draw_byte_time() + stretch;
                    let at = self.sched.now() + dur;
                    self.sched.schedule(at, Job::Advance);
                } else {
                    self.finish_op_completed(stop, buf, len)?;
                }
            }
        }
        Ok(())
    }

    fn finish_op_completed(
        &mut self,
        stop: bool,
        buf: [u8; MAX_OP_BYTES],
        len: u8,
    ) -> Result<(), I2cSimError> {
        if stop {
            self.emit(I2cEvent::Stop)?;
            self.bus_open = false;
            self.last_stop_at = Some(self.sched.now());
        }
        self.master_port.result = Some((Ok(()), buf, len));
        let now = self.sched.now();
        self.sched.schedule(now, Job::PollMaster);
        Ok(())
    }

    fn finish_op(&mut self, result: Result<(), I2cFault>) -> Result<(), I2cSimError> {
        // Fault paths always close the bus (either nobody claimed it, or the
        // slave stopped cooperating mid-write).
        self.emit(I2cEvent::Stop)?;
        self.bus_open = false;
        self.last_stop_at = Some(self.sched.now());
        self.master_port.result = Some((result, [0u8; MAX_OP_BYTES], 0));
        let now = self.sched.now();
        self.sched.schedule(now, Job::PollMaster);
        Ok(())
    }

    pub fn run_until(&mut self, deadline: Nanos) -> Result<I2cRunSummary, I2cSimError> {
        let start = self.sched.now();
        self.sched.schedule(start, Job::PollMaster);

        while let Some((_now, job)) = self.sched.pop_due(deadline) {
            match job {
                Job::PollMaster => self.poll_master()?,
                Job::Advance => self.advance()?,
            }
            if self.master_done
                && self.in_flight.is_none()
                && self.deferred_start.is_none()
                && self.master_port.result.is_none()
            {
                break;
            }
        }

        if !self.ended {
            self.emit(I2cEvent::EndOfObservation)?;
            self.ended = true;
        }

        Ok(I2cRunSummary {
            events: self.trace.len(),
            master_done: self.master_done,
            ended_at: self.sched.now(),
        })
    }
}
