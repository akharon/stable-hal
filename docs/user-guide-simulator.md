# User Guide — Deterministic Simulator & DST Harness (`stablehal-sim`)

Audience: developers testing drivers/protocol stacks on the host, and anyone
extending the harness (new observers, new actors).

## 1. Mental model

The sim is a **two-device world**: device 0 and device 1, joined by two
simplex UART lines (line *i* carries device *i*'s transmissions). One virtual
clock, one event queue, one seeded PRNG. A run is a pure function of
`(SimConfig, seed, actors)` — there is nothing else.

```text
actor 0 ──try_write──► line 0 ──latency/corruption──► actor 1
actor 1 ──try_write──► line 1 ──latency/corruption──► actor 0
                 (every wire event → oracles + trace)
```

## 2. Minimal example

```rust
use stablehal_contract::Nanos;
use stablehal_sim::{Actor, NextPoll, Sim, SimConfig, UartPort};
use stablehal_uart::{UartBus, UartConfig};

struct Hello { sent: bool }
impl Actor for Hello {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        if !self.sent && port.try_write(b'!').is_ok() { self.sent = true; }
        if self.sent { NextPoll::Done } else { NextPoll::At(now + Nanos::from_micros(100)) }
    }
}
struct Sink;
impl Actor for Sink {
    fn poll(&mut self, _now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        while port.try_read().is_some() {}
        NextPoll::OnRx
    }
}

let cfg = SimConfig::new(42, UartConfig::rtu_default(9600));
let mut sim = Sim::new(cfg, Box::new(Hello { sent: false }), Box::new(Sink));
let summary = sim.run_until(Nanos::from_millis(10)).unwrap();
println!("events: {}, hash: {:016x}", summary.events, sim.trace_hash());
```

## 3. `SimConfig` — the environment contract

| Field | Meaning | Default |
|---|---|---|
| `seed` | The run's entire entropy. | — |
| `uart` | Character framing for both lines. | — |
| `latency` | Per-character propagation envelope; drawn per character from the seed. | `1µs..=50µs` |
| `corrupt_ppm` | Per-character corruption probability (ppm). A corrupted character arrives as a random `UartError`. | `0` |

These fields *are* the environment's contract: software that passes a sweep
has been exercised against everything this envelope allows. Widen the
envelope → stronger claim, more schedules to explore.

## 4. Actors and the poll protocol

`Actor::poll(now, port) -> NextPoll` is called:

* at `t = 0`,
* whenever a byte is **delivered** to that actor (always — you never miss RX),
* at any time the actor previously requested via `NextPoll::At(t)`.

Return values: `At(t)` = wake me then (RX still wakes you earlier);
`OnRx` = only RX can advance me; `Done` = never poll again. Multiple
outstanding `At` wake-ups are harmless — drivers are state machines and
re-polling is idempotent.

**Adapting a `no_std` driver** is a ~10-line wrapper; see `SlaveActor` and
`ScriptedMaster` in [runner.rs](../crates/stablehal-conformance/src/runner.rs).
The driver itself must depend only on `UartBus` + the `now` argument.

## 5. Observers: plugging in higher-layer oracles

The engine knows nothing about protocols. Judgement above the UART layer
attaches through `BusObserver`:

```rust
pub trait BusObserver {
    fn on_event(&mut self, at: Nanos, line: u8, event: &LineEvent) -> Result<(), String>;
    fn on_end(&mut self, at: Nanos) -> Result<(), String> { Ok(()) }
}
```

Returning `Err` aborts the run with `SimError::Observer` (trace preserved up
to and including the offending event). For any `Contract`, the generic
`ContractObserver` adapts it: you supply a mapping
`(line, LineEvent) -> Option<C::Event>` plus an optional end-of-observation
event. The ready-made MODBUS instance is
`stablehal_conformance::runner::rtu_observer(timing, judge_master, judge_slave)`.

Observers see **transmitter-side** events (`TxStart`/`TxEnd`) and delivery
events; protocol oracles should map only the transmitter events — judge what
a party emitted, not what the channel did to it.

## 6. Running DST sweeps

Pattern (this is exactly what `examples/modbus-master` does):

```rust
for seed in 0..10_000 {
    let mut cfg = SimConfig::new(seed, uart);
    cfg.corrupt_ppm = 2_000;
    let mut sim = Sim::new(cfg, master_for(seed), slave_for(seed));
    sim.add_observer(rtu_observer(timing, true, true));
    match sim.run_until(Nanos::from_secs(60)) {
        Err(e) => panic!("seed {seed}: {e}"),        // reproduce with this seed
        Ok(s)  => assert!(s.done[0], "seed {seed}: script hung"),
    }
    // + assert your semantic properties (right data or honest error)
}
```

Guidelines:

* **Derive all per-run variation from the seed** (scripts, slave delays):
  `SplitMix64::new(seed ^ SOME_CONST)`. Never from anything else.
* **Two failure classes**: wire violations (oracle) and property failures
  (wrong data). Honest give-ups after retries under fault injection are
  *contract-legal* — count them, don't fail on them.
* **Virtual deadlines are cheap.** `run_until(Nanos::from_secs(60))` costs
  wall-clock time proportional to events, not to virtual time.

## 7. Reproducing a failure

1. The failure report includes the seed (make sure your harness prints it).
2. Re-run with the same seed and config: the violation, its step index, and
   the FNV-1a `trace_hash()` are identical — asserted by
   [dst.rs](../crates/stablehal-sim/tests/dst.rs) and the
   `--replay-check` mode of `modbus-master`.
3. Inspect `sim.trace()`: `&[TraceEvent<(line, LineEvent)>]`, every wire event
   with its virtual timestamp, ending at the offending event.

If a replay ever diverges, that is a **determinism leak** (new entropy source
or iteration-order dependence) — treat it as a P0 bug in the harness, not as
flakiness.

## 8. Extending the engine — rules

* Never read wall time, environment, or thread state inside the engine or an
  actor.
* Any new random choice must draw from the run's `SplitMix64`, at a point in
  the event order (not during setup iteration over a hash map, etc.).
* New scheduled work goes through the queue so the `(at, seq)` total order
  stays authoritative.
* Topology beyond two devices (multi-drop RS-485, I2C shared bus) is handled
  by a **separate `Scheduler`-based world per protocol** (`i2c::I2cWorld`,
  `lin::LinWorld`), not by special-casing a third device into the 2-device
  UART `Sim`. Follow that pattern (architecture.md ADR-6/ADR-10, risk T4) for
  any new multi-drop protocol.
