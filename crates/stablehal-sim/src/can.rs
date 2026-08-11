//! Deterministic CAN bus simulator: N symmetric nodes sharing a
//! `Scheduler`, mirroring [`crate::i2c::I2cWorld`]'s determinism
//! guarantees but with a genuinely different topology (see
//! `stablehal_can::node`'s doc comment): CAN has no master/slave, so
//! `CanWorld` polls every node for an offered frame each bus-idle tick,
//! picks the arbitration winner (lowest `CanId` — standard beats extended,
//! numerically smaller wins; a documented simplification of real bit-level
//! arbitration, same honesty pattern as `CAN-BIT-STUFFING`'s existing
//! note), and lets every *other* node ACK it. `CanEvent::Frame` is judged
//! atomically (no cross-event contract state), so one round is exactly one
//! `emit` — no per-byte state machine.

use std::fmt;

use stablehal_can::contract::{CanContract, CanEvent};
use stablehal_can::node::CanNodeActor;
use stablehal_can::{CanConfig, CanFrame, CanId};
use stablehal_contract::{Nanos, Oracle, TraceEvent};

use crate::sched::Scheduler;

#[derive(Debug)]
pub enum CanSimError {
    Contract { violation: String },
}

impl fmt::Display for CanSimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CanSimError::Contract { violation } => write!(f, "CAN contract violated: {violation}"),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct CanRunSummary {
    pub events: usize,
    pub ended_at: Nanos,
}

pub struct CanWorldConfig {
    pub seed: u64,
    pub config: CanConfig,
    /// Per-frame duration jitter as a fraction of the margined estimate, in
    /// parts per 1000 (e.g. 50 = +/-5%). Drawn once per frame from the
    /// seeded stream — same discipline as `I2cWorldConfig::jitter_permille`.
    /// Real bus-level jitter sources (bit stuffing, inter-frame spacing
    /// variance) aren't modeled individually; this stands in for their
    /// combined effect so seed sweeps actually explore distinct schedules.
    pub jitter_permille: u32,
}

impl CanWorldConfig {
    pub fn new(seed: u64, config: CanConfig) -> Self {
        CanWorldConfig {
            seed,
            config,
            jitter_permille: 50,
        }
    }
}

enum Job {
    PollNodes,
}

/// Arbitration priority key: standard IDs beat extended, numerically
/// smaller wins — `CanId`'s derived `Ord` already ranks variants in
/// declaration order (`Standard` before `Extended`) then by inner value,
/// which is exactly this rule.
fn arbitration_key(frame: &CanFrame) -> CanId {
    frame.id
}

pub struct CanWorld {
    sched: Scheduler<Job>,
    nodes: Vec<Box<dyn CanNodeActor>>,
    config: CanConfig,
    jitter_permille: u32,
    oracle: Oracle<CanContract>,
    trace: Vec<TraceEvent<CanEvent>>,
    ended: bool,
}

impl CanWorld {
    pub fn new(config: CanWorldConfig) -> Self {
        CanWorld {
            sched: Scheduler::new(config.seed),
            nodes: Vec::new(),
            config: config.config,
            jitter_permille: config.jitter_permille,
            oracle: Oracle::new(CanContract::new(config.config)),
            trace: Vec::new(),
            ended: false,
        }
    }

    /// Register a node on the bus. Order matters only for arbitration ties:
    /// when two nodes offer frames with the same `CanId` in one round the
    /// lower registration index wins (the `<` in `poll_round` keeps the
    /// first-seen winner). Two nodes contending the *same* id is itself a
    /// bus-design fault in real CAN; this deterministic tie-break just keeps
    /// the schedule reproducible if it happens.
    pub fn add_node(&mut self, node: Box<dyn CanNodeActor>) {
        self.nodes.push(node);
    }

    pub fn trace(&self) -> &[TraceEvent<CanEvent>] {
        &self.trace
    }
    pub fn trace_hash(&self) -> u64 {
        stablehal_contract::trace::trace_hash(self.trace.iter())
    }
    pub fn now(&self) -> Nanos {
        self.sched.now()
    }

    fn emit(&mut self, event: CanEvent) -> Result<(), CanSimError> {
        let now = self.sched.now();
        self.trace.push(TraceEvent {
            at: now,
            event: event.clone(),
        });
        self.oracle
            .observe(now, &event)
            .map_err(|v| CanSimError::Contract {
                violation: format!("{v}"),
            })
    }

    /// Approximate on-wire duration of `frame`, generously margined for
    /// worst-case bit stuffing (+20%, per ISO 11898-1's own stuffing bound)
    /// since exact stuffed length isn't modeled at this frame granularity,
    /// then jittered from the seeded stream (drawn once per frame, fixed
    /// event order) so seed sweeps explore distinct schedules — mirrors
    /// `I2cWorld::draw_byte_time`.
    fn frame_duration(&mut self, frame: &CanFrame) -> Nanos {
        let id_bits: u64 = match frame.id {
            CanId::Standard(_) => 11 + 1 + 6, // id + RTR + control field
            CanId::Extended(_) => 29 + 2 + 6, // id + SRR/IDE/RTR + control field
        };
        let data_bits = frame.data.len() as u64 * 8;
        // SOF(1) + CRC(15) + CRC delim(1) + ACK slot(1) + ACK delim(1) + EOF(7) + IFS(3).
        let overhead_bits: u64 = 1 + 15 + 1 + 1 + 1 + 7 + 3;
        let total_bits = id_bits + data_bits + overhead_bits;
        let nominal = self
            .config
            .bit_time_ns
            .mul_ratio(total_bits, 1)
            .mul_ratio(6, 5);
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

    fn poll_round(&mut self) -> Result<(), CanSimError> {
        let now = self.sched.now();
        let mut winner: Option<(usize, CanFrame)> = None;
        for (i, node) in self.nodes.iter_mut().enumerate() {
            if let Some(frame) = node.poll_tx(now) {
                let take = match &winner {
                    None => true,
                    Some((_, w)) => arbitration_key(&frame) < arbitration_key(w),
                };
                if take {
                    winner = Some((i, frame));
                }
            }
        }

        match winner {
            Some((tx_idx, frame)) => {
                let mut acked = false;
                for (i, node) in self.nodes.iter_mut().enumerate() {
                    if i == tx_idx {
                        node.on_sent();
                    } else if node.on_frame(&frame) {
                        acked = true;
                    }
                }
                let dur = self.frame_duration(&frame);
                self.emit(CanEvent::Frame {
                    id: frame.id,
                    rtr: frame.rtr,
                    data: frame.data,
                    acked,
                })?;
                self.sched.schedule(now + dur, Job::PollNodes);
            }
            None => {
                // Nobody has anything to send this tick; check back after
                // one bit time so a node that becomes ready later (e.g. a
                // responder right after processing `on_frame`) is found
                // promptly without busy-looping at zero duration.
                self.sched
                    .schedule(now + self.config.bit_time_ns, Job::PollNodes);
            }
        }
        Ok(())
    }

    pub fn run_until(&mut self, deadline: Nanos) -> Result<CanRunSummary, CanSimError> {
        let start = self.sched.now();
        self.sched.schedule(start, Job::PollNodes);

        // Termination: on a bus with driving (finite-script) nodes, stop
        // once *every* driving node is done — the correct rule for several
        // contending testers as well as the reference one-tester topology.
        // With no driving node, fall back to the legacy "stop once any node
        // is done" so pre-existing single-tester traces are unchanged.
        let has_driver = self.nodes.iter().any(|n| n.drives_termination());
        while let Some((_now, Job::PollNodes)) = self.sched.pop_due(deadline) {
            self.poll_round()?;
            let stop = if has_driver {
                self.nodes
                    .iter()
                    .filter(|n| n.drives_termination())
                    .all(|n| n.done())
            } else {
                self.nodes.iter().any(|n| n.done())
            };
            if stop {
                break;
            }
        }

        if !self.ended {
            self.emit(CanEvent::EndOfObservation)?;
            self.ended = true;
        }

        Ok(CanRunSummary {
            events: self.trace.len(),
            ended_at: self.sched.now(),
        })
    }
}
