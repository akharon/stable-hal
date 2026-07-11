//! Hello, StableHAL: two toy devices, one contract, one seed.
//!
//! `Greeter` sends a single byte; `Echoer` acknowledges every byte it
//! receives. A one-clause `Contract` watches line 0 and requires the
//! greeting to be followed by silence before anything else happens on that
//! line. Run it, then rerun the same seed and see the trace hash match
//! exactly — that byte-for-byte reproducibility from a seed is the whole
//! point of StableHAL's simulator.
//!
//! Walked through step by step in `docs/getting-started.md`.

use stablehal::prelude::*;

const UART: UartConfig = UartConfig::rtu_default(9600);
const DEADLINE: Nanos = Nanos::from_millis(10);

// --- Toy contract: nothing may follow the greeting on line 0 --------------

static CL_HELLO_ONCE: Clause = Clause {
    id: "HELLO-ONCE",
    citation: "hello-sim tutorial §1",
    text: "Line 0 must carry exactly one greeting byte and nothing after it.",
};
static HELLO_CLAUSES: [&Clause; 1] = [&CL_HELLO_ONCE];

#[derive(Copy, Clone, Debug)]
enum HelloEvent {
    Greeting,
}

#[derive(Copy, Clone, Debug, Default)]
struct HelloState {
    seen: bool,
}

#[derive(Copy, Clone, Debug)]
struct HelloContract;

impl Contract for HelloContract {
    type State = HelloState;
    type Event = HelloEvent;

    fn initial(&self) -> HelloState {
        HelloState::default()
    }

    fn step(&self, state: &mut HelloState, at: Nanos, event: &HelloEvent) -> Result<(), Breach> {
        let HelloEvent::Greeting = event;
        if state.seen {
            return Err(Breach::timing(
                &CL_HELLO_ONCE,
                at.saturating_sub(Nanos::ZERO),
                TimeBound::exact(Nanos::ZERO),
            ));
        }
        state.seen = true;
        Ok(())
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &HELLO_CLAUSES
    }
}

fn hello_observer() -> ContractObserver<
    HelloContract,
    impl FnMut(u8, &stablehal::uart::LineEvent) -> Option<HelloEvent>,
> {
    ContractObserver::new(
        HelloContract,
        |line, ev| match (line, ev) {
            (0, stablehal::uart::LineEvent::TxStart { .. }) => Some(HelloEvent::Greeting),
            _ => None,
        },
        None,
    )
}

// --- Toy actors -------------------------------------------------------------

/// Sends one byte (0x48, 'H') and then it's done.
struct Greeter {
    sent: bool,
}

impl Actor for Greeter {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        if self.sent {
            return NextPoll::Done;
        }
        match port.try_write(0x48) {
            Ok(()) => {
                self.sent = true;
                NextPoll::Done
            }
            Err(_) => NextPoll::At(now + UART.char_time()),
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

fn run(seed: u64) -> (usize, u64) {
    let mut sim = Sim::new(
        SimConfig::new(seed, UART),
        Box::new(Greeter { sent: false }),
        Box::new(Echoer),
    );
    sim.add_observer(Box::new(hello_observer()));
    let summary = sim.run_until(DEADLINE).expect("hello-sim contract held");
    (summary.events, sim.trace_hash())
}

fn main() {
    let seed = 42;
    let (events, hash) = run(seed);
    println!("seed={seed} events={events} hash={hash:016x}");

    // Same seed, same everything: the simulator is a pure function of
    // (config, seed, actors), so replaying a seed reproduces the run
    // byte-for-byte. This is what makes every DST failure reproducible.
    let (replay_events, replay_hash) = run(seed);
    assert_eq!(events, replay_events);
    assert_eq!(hash, replay_hash);
    println!("replay ok: seed {seed} reproduced identically (hash {replay_hash:016x})");
}
