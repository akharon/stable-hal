//! Deterministic SPI bus simulator: one master actor, N chip-selected slave
//! devices, sharing a `Scheduler`. Mirrors [`crate::i2c::I2cWorld`]'s
//! determinism guarantees and structure but is kept independent per WP2
//! §2.3 (no merged event enum, no shared `Sim` engine).
//!
//! Engine mechanics: SPI's full-duplex byte exchange has no address phase
//! and no ack/stretch, so each in-flight transfer is CsAssert (once) then N
//! byte-timed `ByteXfer` ticks then CsDeAssert — simpler than I2C's
//! Addressing/Write/Read split. A CsAssert is deferred until `cs_idle_min`
//! has elapsed since the previous CsDeAssert, mirroring I2C's
//! `bus_free_min` deferred-Start pattern.

use std::fmt;

use stablehal_contract::{Nanos, Oracle, TraceEvent};
use stablehal_spi::contract::{SpiContract, SpiEvent};
use stablehal_spi::{SpiBusy, SpiConfig, SpiMaster, SpiModeFault, SpiSlave};

use crate::sched::Scheduler;

/// Max bytes per single transfer the sim engine buffers internally.
const MAX_XFER_BYTES: usize = 32;

/// What the master actor wants after a poll — mirrors [`crate::NextPoll`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NextPoll {
    At(Nanos),
    Wait,
    Done,
}

/// Master-side actor: polled like UART/I2C actors; sees an [`SpiMaster`] port.
pub trait SpiMasterActor {
    fn poll(&mut self, now: Nanos, port: &mut dyn SpiMaster) -> NextPoll;
}

/// Adapts a [`stablehal_spi::ScriptedSpiMaster`] into an [`SpiMasterActor`]
/// for use inside [`SpiWorld`].
pub struct SpiMasterAdapter {
    inner: stablehal_spi::ScriptedSpiMaster,
}

impl SpiMasterAdapter {
    pub fn new(inner: stablehal_spi::ScriptedSpiMaster) -> Self {
        SpiMasterAdapter { inner }
    }
    pub fn into_inner(self) -> stablehal_spi::ScriptedSpiMaster {
        self.inner
    }
    pub fn outcomes(&self) -> &[Option<stablehal_spi::SpiStepOutcome>] {
        self.inner.outcomes()
    }
}

impl SpiMasterActor for SpiMasterAdapter {
    fn poll(&mut self, now: Nanos, port: &mut dyn SpiMaster) -> NextPoll {
        let _ = now;
        // Drain every phase transition available this tick — see the
        // identical fix/comment on `I2cMasterAdapter::poll`.
        while self.inner.step(port) {}
        if self.inner.done() {
            NextPoll::Done
        } else {
            NextPoll::Wait
        }
    }
}

#[derive(Debug)]
pub enum SpiSimError {
    Contract { violation: String },
}

impl fmt::Display for SpiSimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpiSimError::Contract { violation } => write!(f, "SPI contract violated: {violation}"),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SpiRunSummary {
    pub events: usize,
    pub master_done: bool,
    pub ended_at: Nanos,
}

pub struct SpiWorldConfig {
    pub seed: u64,
    pub config: SpiConfig,
    /// Per-byte SCLK jitter as a fraction of the tolerance, in parts per
    /// 1000 (e.g. 1000 = the full configured tolerance). Drawn once per
    /// byte from the seeded stream.
    pub jitter_permille: u32,
}

impl SpiWorldConfig {
    pub fn new(seed: u64, config: SpiConfig) -> Self {
        SpiWorldConfig {
            seed,
            config,
            jitter_permille: 500,
        }
    }
}

/// One in-flight transfer, decomposed into a byte-at-a-time plan the engine
/// steps through on the scheduler.
struct InFlight {
    chip: u8,
    buf: [u8; MAX_XFER_BYTES],
    len: u8,
    idx: u8,
}

enum Job {
    PollMaster,
    /// Fire the next byte-phase transition of the in-flight transfer, or
    /// (when nothing is in flight yet) attempt to open a deferred CsAssert
    /// once `cs_idle_min` has elapsed.
    Advance,
}

struct PendingXfer {
    chip: u8,
    buf: [u8; MAX_XFER_BYTES],
    len: u8,
}

#[derive(Default)]
struct MasterPort {
    pending: Option<PendingXfer>,
    busy: bool,
    result: Option<Result<(), SpiModeFault>>,
}

impl SpiMaster for MasterPort {
    fn try_transfer(&mut self, chip: u8, buf: &mut [u8]) -> Result<(), SpiBusy> {
        if self.busy {
            return Err(SpiBusy);
        }
        let mut b = [0u8; MAX_XFER_BYTES];
        let len = buf.len().min(MAX_XFER_BYTES);
        b[..len].copy_from_slice(&buf[..len]);
        self.pending = Some(PendingXfer {
            chip,
            buf: b,
            len: len as u8,
        });
        self.busy = true;
        Ok(())
    }

    fn poll_done(&mut self) -> Option<Result<(), SpiModeFault>> {
        let result = self.result.take()?;
        self.busy = false;
        Some(result)
    }
}

pub struct SpiWorld {
    sched: Scheduler<Job>,
    master_actor: Box<dyn SpiMasterActor>,
    master_port: MasterPort,
    in_flight: Option<InFlight>,
    /// A transfer waiting for `cs_idle_min` to elapse before its CsAssert.
    deferred_select: Option<PendingXfer>,
    slaves: Vec<(u8, Box<dyn SpiSlave>)>,
    config: SpiConfig,
    jitter_permille: u32,
    oracle: Oracle<SpiContract>,
    trace: Vec<TraceEvent<SpiEvent>>,
    master_done: bool,
    /// Virtual time of the most recent CsDeAssert, if any — the earliest a
    /// new CsAssert may legally begin is this plus `cs_idle_min`.
    last_deassert_at: Option<Nanos>,
    ended: bool,
}

impl SpiWorld {
    pub fn new(config: SpiWorldConfig, master: Box<dyn SpiMasterActor>) -> Self {
        SpiWorld {
            sched: Scheduler::new(config.seed),
            master_actor: master,
            master_port: MasterPort::default(),
            in_flight: None,
            deferred_select: None,
            slaves: Vec::new(),
            config: config.config,
            jitter_permille: config.jitter_permille,
            oracle: Oracle::new(SpiContract::new(config.config)),
            trace: Vec::new(),
            master_done: false,
            last_deassert_at: None,
            ended: false,
        }
    }

    /// Register a slave addressed by `chip` (the chip-select id).
    pub fn add_slave(&mut self, chip: u8, dev: Box<dyn SpiSlave>) {
        self.slaves.push((chip, dev));
    }

    pub fn trace(&self) -> &[TraceEvent<SpiEvent>] {
        &self.trace
    }
    pub fn trace_hash(&self) -> u64 {
        stablehal_contract::trace::trace_hash(self.trace.iter())
    }
    pub fn now(&self) -> Nanos {
        self.sched.now()
    }

    fn emit(&mut self, event: SpiEvent) -> Result<(), SpiSimError> {
        let now = self.sched.now();
        self.trace.push(TraceEvent { at: now, event });
        self.oracle
            .observe(now, &event)
            .map_err(|v| SpiSimError::Contract {
                violation: format!("{v}"),
            })
    }

    fn slave_for(&mut self, chip: u8) -> Option<&mut Box<dyn SpiSlave>> {
        self.slaves
            .iter_mut()
            .find(|(c, _)| *c == chip)
            .map(|(_, d)| d)
    }

    /// One byte-time tick, jittered within the configured tolerance from the
    /// seeded stream (drawn in fixed event order, one draw per byte —
    /// mirrors `I2cWorld::draw_byte_time`). Never drops below the nominal
    /// minus tolerance, so the reference master never breaches its own
    /// `SPI-XFER-TIME` envelope.
    fn draw_byte_time(&mut self) -> Nanos {
        let nominal = self.config.sclk_period.mul_ratio(8, 1).as_nanos() as i64;
        let tolerance = self.config.sclk_tolerance.as_nanos() as i64;
        let jitter = self.jitter_permille as i64;
        let roll = if jitter == 0 || tolerance == 0 {
            0
        } else {
            self.sched.prng().range_inclusive(0, 2 * jitter as u64) as i64 - jitter
        };
        let delta = tolerance * roll / 1000;
        Nanos::from_nanos((nominal + delta).max(1) as u64)
    }

    fn poll_master(&mut self) -> Result<(), SpiSimError> {
        if self.master_done {
            return Ok(());
        }
        let now = self.sched.now();
        let next = self.master_actor.poll(now, &mut self.master_port);

        if self.in_flight.is_none() && self.deferred_select.is_none() {
            if let Some(pending) = self.master_port.pending.take() {
                self.request_select(pending)?;
            }
        }

        match next {
            NextPoll::At(t) => self.sched.schedule(t, Job::PollMaster),
            NextPoll::Wait => {}
            NextPoll::Done => self.master_done = true,
        }
        Ok(())
    }

    /// Either open the transfer immediately, or defer it until
    /// `cs_idle_min` has elapsed since the last CsDeAssert. `SpiContract`'s
    /// initial state is `Idle { since: Nanos::ZERO }` (not an `Option`, so
    /// there is no "never claimed yet" exemption the way I2C's
    /// `BusPhase::Free { since: Option<Nanos> }` has) — the very first
    /// CsAssert of any observation is judged as if idle since time zero, so
    /// a run with no prior CsDeAssert must wait out `cs_idle_min` from zero
    /// too, not open immediately.
    fn request_select(&mut self, pending: PendingXfer) -> Result<(), SpiSimError> {
        let now = self.sched.now();
        let ready_at = self.last_deassert_at.unwrap_or(Nanos::ZERO) + self.config.cs_idle_min;
        if now >= ready_at {
            self.open_select(pending)?;
        } else {
            self.deferred_select = Some(pending);
            self.sched.schedule(ready_at, Job::Advance);
        }
        Ok(())
    }

    fn open_select(&mut self, pending: PendingXfer) -> Result<(), SpiSimError> {
        let PendingXfer { chip, buf, len } = pending;
        self.emit(SpiEvent::CsAssert {
            chip,
            mode: self.config.mode,
        })?;
        let mode = self.config.mode;
        if let Some(dev) = self.slave_for(chip) {
            dev.on_select(mode);
        }
        self.in_flight = Some(InFlight {
            chip,
            buf,
            len,
            idx: 0,
        });
        let dur = self.draw_byte_time();
        let at = self.sched.now() + dur;
        self.sched.schedule(at, Job::Advance);
        Ok(())
    }

    fn advance(&mut self) -> Result<(), SpiSimError> {
        if self.in_flight.is_none() {
            if let Some(pending) = self.deferred_select.take() {
                let now = self.sched.now();
                let ready_at =
                    self.last_deassert_at.unwrap_or(Nanos::ZERO) + self.config.cs_idle_min;
                if now >= ready_at {
                    self.open_select(pending)?;
                } else {
                    self.deferred_select = Some(pending);
                    self.sched.schedule(ready_at, Job::Advance);
                }
            }
            return Ok(());
        }

        let InFlight {
            chip,
            buf,
            len,
            idx,
        } = self.in_flight.take().unwrap();
        if idx < len {
            let mosi = buf[idx as usize];
            let miso = if let Some(dev) = self.slave_for(chip) {
                dev.xfer_byte(mosi)
            } else {
                0xFF
            };
            self.emit(SpiEvent::ByteXfer { chip, mosi, miso })?;
            self.in_flight = Some(InFlight {
                chip,
                buf,
                len,
                idx: idx + 1,
            });
            let dur = self.draw_byte_time();
            let at = self.sched.now() + dur;
            self.sched.schedule(at, Job::Advance);
        } else {
            self.finish_transfer(chip)?;
        }
        Ok(())
    }

    fn finish_transfer(&mut self, chip: u8) -> Result<(), SpiSimError> {
        self.emit(SpiEvent::CsDeAssert { chip })?;
        if let Some(dev) = self.slave_for(chip) {
            dev.on_deselect();
        }
        self.last_deassert_at = Some(self.sched.now());
        self.master_port.result = Some(Ok(()));
        let now = self.sched.now();
        self.sched.schedule(now, Job::PollMaster);
        Ok(())
    }

    pub fn run_until(&mut self, deadline: Nanos) -> Result<SpiRunSummary, SpiSimError> {
        let start = self.sched.now();
        self.sched.schedule(start, Job::PollMaster);

        while let Some((_now, job)) = self.sched.pop_due(deadline) {
            match job {
                Job::PollMaster => self.poll_master()?,
                Job::Advance => self.advance()?,
            }
            if self.master_done
                && self.in_flight.is_none()
                && self.deferred_select.is_none()
                && self.master_port.result.is_none()
            {
                break;
            }
        }

        if !self.ended {
            self.emit(SpiEvent::EndOfObservation)?;
            self.ended = true;
        }

        Ok(SpiRunSummary {
            events: self.trace.len(),
            master_done: self.master_done,
            ended_at: self.sched.now(),
        })
    }
}
