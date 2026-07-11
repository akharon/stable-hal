//! Deterministic simulator + DST harness.
//!
//! The world model: two devices (0 and 1) joined by two simplex UART lines.
//! Device *i* transmits on line *i* and receives from line *1−i*.
//!
//! Determinism guarantees:
//! * one virtual clock, no wall time anywhere;
//! * one seeded [`SplitMix64`] stream is the only source of randomness —
//!   latencies inside the environment envelope, fault injection choices;
//! * the event queue breaks timestamp ties by insertion sequence number.
//!
//! Consequence: a run is a pure function of (config, seed, actors), and any
//! failure replays byte-for-byte from its seed. The environment itself is
//! conforming by construction: the simulated wire physically cannot emit a
//! character pattern the [`LineContract`] forbids, and every wire event is
//! still fed through online oracles so the claim is *checked*, not assumed.
//!
//! # Example
//!
//! Two toy actors — one that sends a byte, one that echoes it back — driven
//! to completion, with the same seed replayed to show the trace hash matches
//! byte-for-byte. See `docs/getting-started.md` for a walkthrough that adds a
//! contract observer on top of this shape.
//!
//! ```
//! use stablehal_contract::Nanos;
//! use stablehal_sim::{Actor, NextPoll, Sim, SimConfig, UartPort};
//! use stablehal_uart::{UartBus, UartConfig};
//!
//! const UART: UartConfig = UartConfig::rtu_default(9600);
//!
//! struct Sender { sent: bool }
//! impl Actor for Sender {
//!     fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
//!         if self.sent {
//!             return NextPoll::Done;
//!         }
//!         match port.try_write(0x48) {
//!             Ok(()) => { self.sent = true; NextPoll::Done }
//!             Err(_) => NextPoll::At(now + UART.char_time()),
//!         }
//!     }
//! }
//!
//! struct Echoer;
//! impl Actor for Echoer {
//!     fn poll(&mut self, _now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
//!         while let Some(Ok(_byte)) = port.try_read() {
//!             let _ = port.try_write(0xEE);
//!         }
//!         NextPoll::OnRx
//!     }
//! }
//!
//! let run = |seed| {
//!     let mut sim = Sim::new(SimConfig::new(seed, UART), Box::new(Sender { sent: false }), Box::new(Echoer));
//!     sim.run_until(Nanos::from_millis(10)).unwrap();
//!     sim.trace_hash()
//! };
//!
//! assert_eq!(run(42), run(42)); // same seed ⇒ same hash, always
//! ```

#![deny(unsafe_code)]

pub mod can;
pub mod i2c;
pub mod lin;
mod sched;
pub mod spi;

use std::collections::VecDeque;
use std::fmt;

use sched::Scheduler;
use stablehal_contract::{Nanos, Oracle, TimeBound, TraceEvent, Violation};
use stablehal_uart::{LineContract, LineEvent, TxBusy, UartBus, UartConfig, UartError};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The *environment contract*: the envelope of behaviors the physical channel
/// is allowed to exhibit. Software tested under DST is exercised against this
/// whole envelope, not a single happy-path trace.
#[derive(Copy, Clone, Debug)]
pub struct SimConfig {
    pub seed: u64,
    pub uart: UartConfig,
    /// Propagation delay envelope per character (receiver sees the byte this
    /// long after the transmitter releases it).
    pub latency: TimeBound,
    /// Per-character corruption probability, in parts per million.
    pub corrupt_ppm: u32,
    /// Use boundary-biased envelope sampling (25% min, 25% max, 50% interior)
    /// instead of uniform. Default `false` — flipping changes all trace hashes.
    /// WP4 §4.2.
    pub boundary_bias: bool,
}

impl SimConfig {
    pub fn new(seed: u64, uart: UartConfig) -> Self {
        SimConfig {
            seed,
            uart,
            latency: TimeBound::new(Nanos::from_micros(1), Nanos::from_micros(50)),
            corrupt_ppm: 0,
            boundary_bias: false,
        }
    }

    /// Builder-style alternative to setting `latency` after `new`.
    pub fn with_latency(mut self, latency: TimeBound) -> Self {
        self.latency = latency;
        self
    }

    /// Builder-style alternative to setting `corrupt_ppm` after `new`.
    pub fn with_corrupt_ppm(mut self, ppm: u32) -> Self {
        self.corrupt_ppm = ppm;
        self
    }

    /// Builder-style alternative to setting `boundary_bias` after `new`.
    pub fn with_boundary_bias(mut self, bias: bool) -> Self {
        self.boundary_bias = bias;
        self
    }
}

// ---------------------------------------------------------------------------
// Actor interface
// ---------------------------------------------------------------------------

/// What an actor wants after a poll.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NextPoll {
    /// Poll me again at (or after) this virtual time. Arriving bytes always
    /// trigger an extra poll regardless.
    At(Nanos),
    /// Only incoming bytes can advance me.
    OnRx,
    /// I have finished; never poll me again.
    Done,
}

/// A device under simulation. Implementations adapt real `no_std` drivers
/// (written against [`UartBus`] only) into the sim's poll loop.
pub trait Actor {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll;
}

/// The [`UartBus`] view an actor sees during one poll. Writes are staged and
/// turned into wire events by the engine after the poll returns.
pub struct UartPort<'a> {
    now: Nanos,
    char_time: Nanos,
    tx_busy_until: &'a mut Nanos,
    rx_queue: &'a mut VecDeque<Result<u8, UartError>>,
    staged_tx: &'a mut Vec<u8>,
}

impl UartBus for UartPort<'_> {
    fn try_write(&mut self, byte: u8) -> Result<(), TxBusy> {
        if self.now < *self.tx_busy_until {
            return Err(TxBusy);
        }
        *self.tx_busy_until = self.now + self.char_time;
        self.staged_tx.push(byte);
        Ok(())
    }

    fn try_read(&mut self) -> Option<Result<u8, UartError>> {
        self.rx_queue.pop_front()
    }
}

// ---------------------------------------------------------------------------
// Bus observers (higher-layer oracles, e.g. the MODBUS RTU contract)
// ---------------------------------------------------------------------------

/// Judgement hook over the raw wire. Line 0 carries device 0 → device 1,
/// line 1 the reverse. Return `Err` to abort the run with a violation.
pub trait BusObserver {
    fn on_event(&mut self, at: Nanos, line: u8, event: &LineEvent) -> Result<(), String>;
    /// Called once when the run ends, with a timestamp past all activity —
    /// lets frame-oriented contracts finalize a trailing frame.
    fn on_end(&mut self, at: Nanos) -> Result<(), String> {
        let _ = at;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Errors and summaries
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SimError {
    /// The UART line contract itself was violated (should be impossible for
    /// the built-in wire — this is the self-check).
    Line {
        line: u8,
        violation: Violation<LineEvent>,
    },
    /// A higher-layer observer (e.g. the RTU contract oracle) found a breach.
    Observer { message: String },
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::Line { line, violation } => {
                write!(f, "line {} contract violated: {}", line, violation)
            }
            SimError::Observer { message } => write!(f, "observer violation: {}", message),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct RunSummary {
    /// Total wire events recorded.
    pub events: usize,
    /// Per-actor completion (an always-listening device never reports Done).
    pub done: [bool; 2],
    /// True when every actor reported `Done` before the deadline.
    pub all_done: bool,
    /// Virtual time at which the run stopped.
    pub ended_at: Nanos,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Job {
    Poll(usize),
    TxEnd {
        line: usize,
        byte: u8,
    },
    Deliver {
        line: usize,
        payload: Result<u8, UartError>,
    },
}

struct PortCore {
    tx_busy_until: Nanos,
    rx_queue: VecDeque<Result<u8, UartError>>,
}

pub struct Sim {
    config: SimConfig,
    char_time: Nanos,
    sched: Scheduler<Job>,
    ports: [PortCore; 2],
    actors: [Box<dyn Actor>; 2],
    done: [bool; 2],
    line_oracles: [Oracle<LineContract>; 2],
    observers: Vec<Box<dyn BusObserver>>,
    trace: Vec<TraceEvent<(u8, LineEvent)>>,
    staged_tx: Vec<u8>,
}

impl Sim {
    /// `actors[i]` transmits on line `i` and receives from line `1−i`.
    pub fn new(config: SimConfig, actor0: Box<dyn Actor>, actor1: Box<dyn Actor>) -> Self {
        let contract = LineContract::new(config.uart);
        Sim {
            char_time: config.uart.char_time(),
            sched: Scheduler::new(config.seed),
            ports: [
                PortCore {
                    tx_busy_until: Nanos::ZERO,
                    rx_queue: VecDeque::new(),
                },
                PortCore {
                    tx_busy_until: Nanos::ZERO,
                    rx_queue: VecDeque::new(),
                },
            ],
            actors: [actor0, actor1],
            done: [false, false],
            line_oracles: [Oracle::new(contract), Oracle::new(contract)],
            observers: Vec::new(),
            trace: Vec::new(),
            staged_tx: Vec::new(),
            config,
        }
    }

    pub fn add_observer(&mut self, observer: Box<dyn BusObserver>) {
        self.observers.push(observer);
    }

    pub fn config(&self) -> &SimConfig {
        &self.config
    }
    pub fn now(&self) -> Nanos {
        self.sched.now()
    }
    pub fn trace(&self) -> &[TraceEvent<(u8, LineEvent)>] {
        &self.trace
    }
    /// Deterministic fingerprint of everything that happened on the wire.
    pub fn trace_hash(&self) -> u64 {
        stablehal_contract::trace::trace_hash(self.trace.iter())
    }

    /// Record a wire event, run it through the line oracle and observers.
    fn emit(&mut self, line: usize, event: LineEvent) -> Result<(), SimError> {
        let now = self.sched.now();
        self.trace.push(TraceEvent {
            at: now,
            event: (line as u8, event),
        });
        self.line_oracles[line]
            .observe(now, &event)
            .map_err(|violation| SimError::Line {
                line: line as u8,
                violation,
            })?;
        for obs in &mut self.observers {
            obs.on_event(now, line as u8, &event)
                .map_err(|message| SimError::Observer { message })?;
        }
        Ok(())
    }

    fn poll_actor(&mut self, i: usize) -> Result<(), SimError> {
        if self.done[i] {
            return Ok(());
        }
        debug_assert!(self.staged_tx.is_empty());
        let now = self.sched.now();
        let next = {
            let port_core = &mut self.ports[i];
            let mut port = UartPort {
                now,
                char_time: self.char_time,
                tx_busy_until: &mut port_core.tx_busy_until,
                rx_queue: &mut port_core.rx_queue,
                staged_tx: &mut self.staged_tx,
            };
            self.actors[i].poll(now, &mut port)
        };
        // Turn staged writes into wire events. `try_write` allows at most one
        // pending character, so at most one byte is staged per poll. Move the
        // buffer out (an empty `Vec` allocates nothing) so `emit` can borrow
        // `self`, then hand the same allocation back — no per-poll heap churn
        // in the hot sweep path.
        let mut staged = std::mem::take(&mut self.staged_tx);
        for &byte in &staged {
            self.emit(i, LineEvent::TxStart { byte })?;
            self.sched
                .schedule(now + self.char_time, Job::TxEnd { line: i, byte });
        }
        staged.clear();
        self.staged_tx = staged;
        match next {
            NextPoll::At(t) => {
                self.sched.schedule(t, Job::Poll(i));
            }
            NextPoll::OnRx => {}
            NextPoll::Done => self.done[i] = true,
        }
        Ok(())
    }

    /// Run until the deadline (or both actors are done). On violation the
    /// run aborts; the trace up to and including the offending event is kept.
    pub fn run_until(&mut self, deadline: Nanos) -> Result<RunSummary, SimError> {
        let start = self.sched.now();
        self.sched.schedule(start, Job::Poll(0));
        self.sched.schedule(start, Job::Poll(1));

        while let Some((now, job)) = self.sched.pop_due(deadline) {
            match job {
                Job::Poll(i) => self.poll_actor(i)?,
                Job::TxEnd { line, byte } => {
                    self.emit(line, LineEvent::TxEnd { byte })?;
                    // Environment nondeterminism, drawn from the seeded
                    // stream *in event order*: propagation latency, then
                    // (optionally) corruption.
                    let boundary_bias = self.config.boundary_bias;
                    let corrupt_ppm = self.config.corrupt_ppm;
                    let latency_bound = self.config.latency;
                    let prng = self.sched.prng();
                    let latency = if boundary_bias {
                        prng.pick_biased(latency_bound)
                    } else {
                        prng.pick(latency_bound)
                    };
                    let corrupt = corrupt_ppm > 0 && prng.chance(u64::from(corrupt_ppm), 1_000_000);
                    let payload = if corrupt {
                        let kind = match prng.range_inclusive(0, 2) {
                            0 => UartError::Framing,
                            1 => UartError::Parity,
                            _ => UartError::Noise,
                        };
                        Err(kind)
                    } else {
                        Ok(byte)
                    };
                    self.sched
                        .schedule(now + latency, Job::Deliver { line, payload });
                }
                Job::Deliver { line, payload } => {
                    let ev = match payload {
                        Ok(byte) => LineEvent::Delivered { byte },
                        Err(error) => LineEvent::DeliveredCorrupt { error },
                    };
                    self.emit(line, ev)?;
                    let receiver = 1 - line;
                    self.ports[receiver].rx_queue.push_back(payload);
                    self.sched.schedule(now, Job::Poll(receiver));
                }
            }
            if self.done == [true, true] {
                break;
            }
        }

        let end_at = self.sched.now() + self.char_time.mul_ratio(7, 2) + Nanos::from_millis(1);
        for obs in &mut self.observers {
            obs.on_end(end_at)
                .map_err(|message| SimError::Observer { message })?;
        }

        Ok(RunSummary {
            events: self.trace.len(),
            done: self.done,
            all_done: self.done == [true, true],
            ended_at: self.sched.now(),
        })
    }
}

/// Adapt any `Contract` over `(line, LineEvent)`-derivable events into a
/// [`BusObserver`] by mapping wire events through `f`.
pub struct ContractObserver<C, F>
where
    C: stablehal_contract::Contract,
    F: FnMut(u8, &LineEvent) -> Option<C::Event>,
{
    oracle: Oracle<C>,
    map: F,
    /// Event emitted on `on_end`, if the contract wants finalization.
    end_event: Option<C::Event>,
}

impl<C, F> ContractObserver<C, F>
where
    C: stablehal_contract::Contract,
    F: FnMut(u8, &LineEvent) -> Option<C::Event>,
{
    pub fn new(contract: C, map: F, end_event: Option<C::Event>) -> Self {
        ContractObserver {
            oracle: Oracle::new(contract),
            map,
            end_event,
        }
    }
}

impl<C, F> BusObserver for ContractObserver<C, F>
where
    C: stablehal_contract::Contract,
    F: FnMut(u8, &LineEvent) -> Option<C::Event>,
{
    fn on_event(&mut self, at: Nanos, line: u8, event: &LineEvent) -> Result<(), String> {
        if let Some(ev) = (self.map)(line, event) {
            self.oracle.observe(at, &ev).map_err(|v| format!("{}", v))?;
        }
        Ok(())
    }

    fn on_end(&mut self, at: Nanos) -> Result<(), String> {
        if let Some(ev) = self.end_event.take() {
            self.oracle.observe(at, &ev).map_err(|v| format!("{}", v))?;
        }
        Ok(())
    }
}
