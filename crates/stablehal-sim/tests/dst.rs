//! M1 exit criterion: the DST harness must
//! 1. find a planted, environment-dependent timing bug in a toy driver by
//!    sweeping seeds, and
//! 2. reproduce the failure byte-for-byte when the seed is replayed.
//!
//! The toy protocol: device 0 ("pinger") sends 1-byte frames that device 1
//! ("echoer") acknowledges. The toy contract demands ≥ GAP silence between
//! the pinger's frames. The planted bug: the pinger schedules the next frame
//! relative to *ack arrival* with an optimistic slack estimate, so the wire
//! gap only shrinks below GAP when both propagation latencies happen to be
//! drawn small — exactly the class of bug a single mock trace never shows.

use stablehal_contract::{Breach, Clause, Contract, Nanos, TimeBound};
use stablehal_sim::{Actor, ContractObserver, NextPoll, Sim, SimConfig, UartPort};
use stablehal_uart::{LineEvent, UartBus, UartConfig};

const UART: UartConfig = UartConfig::rtu_default(9600);
const GAP: Nanos = Nanos::from_millis(5);
const PINGS: u32 = 20;
const DEADLINE: Nanos = Nanos::from_millis(400);

// --- Toy contract: min inter-frame gap on the pinger's line ---------------

static CL_TOY_GAP: Clause = Clause {
    id: "TOY-MIN-GAP",
    citation: "toy ping protocol §1",
    text: "Consecutive pinger frames must be separated by at least GAP of silence.",
};
static TOY_CLAUSES: [&Clause; 1] = [&CL_TOY_GAP];

#[derive(Copy, Clone, Debug)]
enum GapEvent {
    Start,
    End,
}

#[derive(Copy, Clone, Debug)]
struct GapContract {
    gap: Nanos,
}

#[derive(Clone, Debug)]
struct GapState {
    last_end: Option<Nanos>,
}

impl Contract for GapContract {
    type State = GapState;
    type Event = GapEvent;

    fn initial(&self) -> GapState {
        GapState { last_end: None }
    }

    fn step(&self, state: &mut GapState, at: Nanos, event: &GapEvent) -> Result<(), Breach> {
        match event {
            GapEvent::Start => {
                if let Some(end) = state.last_end {
                    let silence = at.saturating_sub(end);
                    if silence < self.gap {
                        return Err(Breach::timing(
                            &CL_TOY_GAP,
                            silence,
                            TimeBound::at_least(self.gap),
                        ));
                    }
                }
                Ok(())
            }
            GapEvent::End => {
                state.last_end = Some(at);
                Ok(())
            }
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &TOY_CLAUSES
    }
}

fn gap_observer() -> ContractObserver<GapContract, impl FnMut(u8, &LineEvent) -> Option<GapEvent>> {
    ContractObserver::new(
        GapContract { gap: GAP },
        |line, ev| match (line, ev) {
            (0, LineEvent::TxStart { .. }) => Some(GapEvent::Start),
            (0, LineEvent::TxEnd { .. }) => Some(GapEvent::End),
            _ => None,
        },
        None,
    )
}

// --- Toy actors -------------------------------------------------------------

enum PingState {
    SendAt(Nanos),
    AwaitAck,
    Done,
}

struct Pinger {
    sent: u32,
    /// Optimistic slack the driver subtracts from its wait, assuming the ack
    /// round trip "always" takes at least this long. slack = 0 is correct.
    slack: Nanos,
    state: PingState,
}

impl Pinger {
    fn new(slack: Nanos) -> Self {
        Pinger {
            sent: 0,
            slack,
            state: PingState::SendAt(GAP),
        }
    }
}

impl Actor for Pinger {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        while let Some(rx) = port.try_read() {
            if rx.is_ok() {
                if let PingState::AwaitAck = self.state {
                    if self.sent >= PINGS {
                        self.state = PingState::Done;
                    } else {
                        // BUG (when slack > 0): waits GAP − slack from ack
                        // arrival; the real wire silence is only ≥ GAP if the
                        // round trip took ≥ slack.
                        self.state = PingState::SendAt(now + GAP.saturating_sub(self.slack));
                    }
                }
            }
        }
        match self.state {
            PingState::SendAt(t) => {
                if now >= t {
                    if port.try_write(self.sent as u8).is_ok() {
                        self.sent += 1;
                        self.state = PingState::AwaitAck;
                        return NextPoll::OnRx;
                    }
                    NextPoll::At(now + UART.char_time())
                } else {
                    NextPoll::At(t)
                }
            }
            PingState::AwaitAck => NextPoll::OnRx,
            PingState::Done => NextPoll::Done,
        }
    }
}

/// Acknowledges every received byte with 0xEE.
struct Echoer;

impl Actor for Echoer {
    fn poll(&mut self, _now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        while let Some(rx) = port.try_read() {
            if rx.is_ok() {
                let _ = port.try_write(0xEE);
            }
        }
        NextPoll::OnRx
    }
}

fn run(seed: u64, slack: Nanos) -> (Result<(), String>, u64) {
    let mut sim = Sim::new(
        SimConfig::new(seed, UART),
        Box::new(Pinger::new(slack)),
        Box::new(Echoer),
    );
    sim.add_observer(Box::new(gap_observer()));
    let outcome = sim
        .run_until(DEADLINE)
        .map(|_| ())
        .map_err(|e| e.to_string());
    (outcome, sim.trace_hash())
}

// The bug fires only when latency₀ + latency₁ < slack − char_time; with the
// default 1–50 µs envelope and this slack it is a ~1% event per ping.
fn buggy_slack() -> Nanos {
    UART.char_time() + Nanos::from_micros(8)
}

#[test]
fn correct_driver_survives_the_whole_envelope() {
    for seed in 0..100 {
        let (outcome, _) = run(seed, Nanos::ZERO);
        assert!(
            outcome.is_ok(),
            "seed {} unexpectedly failed: {:?}",
            seed,
            outcome
        );
    }
}

#[test]
fn dst_sweep_finds_planted_timing_bug_and_seed_replays_exactly() {
    let mut failing: Option<(u64, String, u64)> = None;
    for seed in 0..200 {
        let (outcome, hash) = run(seed, buggy_slack());
        if let Err(msg) = outcome {
            assert!(
                msg.contains("TOY-MIN-GAP"),
                "wrong clause reported: {}",
                msg
            );
            failing = Some((seed, msg, hash));
            break;
        }
    }
    let (seed, msg, hash) = failing.expect("sweep failed to find the planted bug in 200 seeds");

    // Replay: identical violation message AND identical wire trace hash.
    let (replay_outcome, replay_hash) = run(seed, buggy_slack());
    assert_eq!(replay_outcome.unwrap_err(), msg);
    assert_eq!(
        replay_hash, hash,
        "seed replay diverged — nondeterminism leak"
    );
}

#[test]
fn distinct_seeds_explore_distinct_schedules() {
    let (_, h0) = run(0, Nanos::ZERO);
    let (_, h1) = run(1, Nanos::ZERO);
    assert_ne!(h0, h1);
}
