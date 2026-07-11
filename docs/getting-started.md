# Getting Started

A linear path from `git clone` to your own actor running under deterministic
simulation testing (DST) with a contract watching the wire. If you want the
*why* behind any of this, see [the idea in one paragraph](../README.md#the-idea-in-one-paragraph)
and [docs/architecture.md](architecture.md); this document is purely the *how*.

## 1. What you'll do

Run the two demo binaries, then write a ~40-line pair of simulated devices
and watch a one-clause contract judge the wire between them. Finally, plant a
bug, find it by sweeping seeds, and replay the failing seed to see the exact
same trace hash come back — that byte-for-byte reproducibility from a single
`u64` seed is the core guarantee this project is built around.

## 2. Prerequisites

A stable Rust toolchain, version 1.96 or newer (`rustc --version`). Nothing
else — every crate under `crates/`, `examples/`, and `tools/` builds from the
standard registry with no network fetch. (The one exception is
`adapters/rmodbus-slave`, an optional third-party-stack experiment that pulls
in the `rmodbus` crate; you don't need it for this tutorial.)

## 3. Clone and smoke-test

```sh
cargo test
```

This builds and tests the whole workspace. "Green" means every crate's unit
tests, the MODBUS RTU closed-loop test, and the DST harness self-tests all
pass — a few hundred tests, a few seconds on a modern machine.

## 4. Run the demos

```sh
cargo run --release -p buggy-slave
```

Runs the 13-case conformance suite against a reference MODBUS RTU slave (13/13
pass) and against six deliberately broken slaves, each of which fails with
the specific clause it breaks named in the output (e.g. `RTU-CRC`,
`RTU-T35-MIN`). This is the "hardware certification" side of the project: the
exact same suite later runs against real silicon.

```sh
cargo run --release -p modbus-master -- --seeds 1000
```

Runs the real MODBUS RTU master driver 1,000 times, each run using a
different seed to draw a different (but contract-legal) wire environment —
latency jitter, character corruption. Every wire event is judged online by
the UART line contract and the RTU contract; the summary reports how many
distinct schedules were explored, how many events were judged, and confirms
zero violations and zero silent data corruptions. Add `--help` to either
binary to see all flags.

The number printed after `hash` (or in `--replay-check` output) is the trace
hash: an FNV-1a fingerprint of everything that happened on the wire. Two runs
with the same seed always produce the same hash — that's what
`--replay-check` demonstrates:

```sh
cargo run --release -p modbus-master -- --replay-check 4242
```

## 5. Hello, simulator

This is the core of the tutorial. `examples/hello-sim/src/main.rs` is a
complete, minimal simulation: two actors talking over a simulated UART line,
judged by a one-clause contract. Read it alongside this section, or just run
it:

```sh
cargo run -p hello-sim
```

Expected output: an event count and a trace hash, then a second run of the
same seed reproducing the identical hash.

**The actor interface.** Everything in the simulator is an
[`Actor`](../crates/stablehal-sim/src/lib.rs):

```rust,ignore
pub trait Actor {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll;
}
```

`poll` is called whenever the actor might have something to do. It reads
available bytes off `port` (`port.try_read()`), optionally stages a byte to
send (`port.try_write(byte)`), and returns a [`NextPoll`](../crates/stablehal-sim/src/lib.rs)
telling the engine when to call it again:

* `NextPoll::At(t)` — call me again at virtual time `t` even if nothing else happens.
* `NextPoll::OnRx` — call me again only when a byte arrives.
* `NextPoll::Done` — I'm finished; stop polling me.

`hello-sim`'s `Greeter` sends one byte and returns `Done`; its `Echoer` acks
every byte it receives and always waits for the next one (`OnRx`).

**Wiring it up.**

```rust,ignore
let mut sim = Sim::new(
    SimConfig::new(seed, UART),
    Box::new(Greeter { sent: false }),
    Box::new(Echoer),
);
let summary = sim.run_until(DEADLINE).unwrap();
println!("events={} hash={:016x}", summary.events, sim.trace_hash());
```

`SimConfig::new(seed, uart_config)` is the whole environment: one seed drives
every latency draw and fault-injection choice for the entire run, via a
single seeded `SplitMix64` stream. `Sim::new` takes the config plus the two
actors (actor 0 transmits on line 0, actor 1 on line 1, each receives the
other's line). `run_until(deadline)` drives the event loop until the deadline
or both actors report `Done`. `sim.trace_hash()` is the fingerprint used for
replay checks throughout this project.

## 6. Make a contract watch the wire

A [`Contract`](../crates/stablehal-contract/src/oracle.rs) is a deterministic
state machine over timestamped events: `initial()` gives the starting state,
`step(state, at, event)` advances it or returns a `Breach` naming the broken
clause. `hello-sim` includes one (`HelloContract`, `HELLO-ONCE`): it requires
line 0 to carry exactly one byte. Attaching it to a running simulation is one
call:

```rust,ignore
sim.add_observer(Box::new(ContractObserver::new(
    HelloContract,
    |line, ev| match (line, ev) {
        (0, LineEvent::TxStart { .. }) => Some(HelloEvent::Greeting),
        _ => None,
    },
    None, // no synthetic "end of run" event
)));
```

The middle closure maps raw wire events (which line, what kind of line event)
to your contract's own event type — everything that doesn't matter to this
contract maps to `None` and is ignored.

To see a contract actually catch a bug: `crates/stablehal-sim/tests/dst.rs`
plants a timing bug in a toy driver (it schedules its next send too
optimistically relative to when an ack arrives) and sweeps 200 seeds looking
for a wire gap narrower than the contract allows. The sweep finds a failing
seed, and replaying that exact seed reproduces the identical violation
message and trace hash — read that test for the full pattern (seed sweep →
find failure → replay → assert identical hash). That loop (sweep, find,
replay, fix, sweep again) is the day-to-day workflow this project is built
around.

## 7. Certify an implementation

The conformance suite runs the same kind of judging against a black-box
implementation — useful for third-party stacks or (eventually) real
hardware:

```rust,ignore
use stablehal_conformance::runner::{run_suite, SlaveActor, SuiteConfig};
use stablehal_conformance::{pattern, DUT_UNIT, STANDARD_SUITE};
use stablehal_modbus::{RegisterBank, SlaveBugs, SlaveConfig};

let cfg = SuiteConfig::default_9600();
let report = run_suite(&cfg, STANDARD_SUITE, || {
    Box::new(SlaveActor::new(
        SlaveConfig {
            unit: DUT_UNIT,
            timing: cfg.timing,
            response_delay: stablehal_contract::Nanos::from_millis(1),
            bugs: SlaveBugs::default(),
        },
        RegisterBank::with_pattern(pattern),
    ))
});
assert!(report.all_pass(), "{}", report.failure_text());
```

Two preconditions the suite assumes, or every case fails on a mismatch
instead of the property it's actually testing: the device under test must
answer unit address `DUT_UNIT` (`0x11`), and its holding registers must read
back `pattern(addr) = 0xA000 | addr` before each case runs. See
[docs/user-guide-conformance.md](user-guide-conformance.md) for the full
picture, including how to read a failure report and add new cases.

## 8. Where to go next

Recommended reading order from here:

1. This tutorial (done).
2. The protocol user guide for what you're integrating —
   [uart](user-guide-uart.md), [simulator](user-guide-simulator.md),
   [modbus](user-guide-modbus.md), [i2c](user-guide-i2c.md),
   [lin](user-guide-lin.md), [conformance](user-guide-conformance.md), or
   [containment](user-guide-containment.md).
3. [docs/architecture.md](architecture.md) for the *why*: the full arc42
   architecture, ADRs, and the engineering ground rules.
4. `cargo doc --open -p stablehal` for full API reference, or scope it to one
   crate, e.g. `cargo doc --open -p stablehal-sim`.

If you're integrating your own driver or device rather than following along,
jump straight to the [three integration paths](../README.md#integrating-your-own-driver-or-device-three-paths)
table in the README.
