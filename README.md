# StableHAL

**A deterministic, contract-enforcing HAL + integrated simulation environment.**
The HAL boundary becomes an executable contract; simulation explores the whole
agreed envelope deterministically; hardware is certified against the *same*
oracle. The recurring integration test collapses into a one-time
per-hardware-revision conformance certification.

Pilot protocol: **MODBUS RTU over UART** (FC 03/06/10 subset, unicast).
See [paper/whitepaper.md](paper/whitepaper.md) for the full argument, related
work, traceability table, and measured results.

## Documentation

| Document | Contents |
|---|---|
| [docs/architecture.md](docs/architecture.md) | arc42 architecture specification (context, building blocks, runtime, deployment — with mermaid diagrams), ADRs, risks, glossary |
| [docs/user-guide-uart.md](docs/user-guide-uart.md) | Using `UartBus`, driver conventions, the line contract, backend checklist |
| [docs/user-guide-simulator.md](docs/user-guide-simulator.md) | Sim & DST harness: actors, observers, sweeps, reproducing failures |
| [docs/user-guide-modbus.md](docs/user-guide-modbus.md) | Master/slave drivers, PDU toolbox, the 12-clause RTU wire contract |
| [docs/user-guide-conformance.md](docs/user-guide-conformance.md) | Running/reading the suite, certifying a slave, adding cases |
| [docs/design-spec-next.md](docs/design-spec-next.md) | Design & interface spec for the next milestones (rmodbus adapter, I2C, hardware phase) |
| [next_steps.md](next_steps.md) | Ordered implementation backlog with acceptance criteria (handoff document) |
| [ChangeLog.txt](ChangeLog.txt) | Versioned change history, incl. contract versions |

## The idea in one paragraph

A plain HAL is a signature: it binds neither software nor hardware to any
behavior, timing, or fault envelope — so simulation and hardware drift apart
and every release needs an integration test. StableHAL writes the agreement
down as a deterministic state machine whose timing terms are envelopes
(`min..=max`) and whose faults form a closed vocabulary: hardware's real
nondeterminism becomes *bounded nondeterminism inside the contract*. One
contract yields three artifacts: a reference simulation (conforming by
construction), a `no_std` trace oracle (judges simulated runs and hardware
captures alike), and a generated conformance suite. Software passes
deterministic simulation testing (DST) across the whole envelope; hardware
passes the suite; interoperation follows by transitivity — to the extent the
contract is complete, which is the honest, stated limit.

## Quick start

```sh
cargo test                                        # full verification (all crates)
cargo run --release -p buggy-slave                # closed-loop conformance demo
cargo run --release -p modbus-master -- --seeds 10000   # DST envelope sweep
cargo run --release -p modbus-master -- --replay-check 4242  # determinism proof
```

Expected: the reference slave passes 13/13 conformance cases while six
deliberately buggy slaves each fail with the exact breached clause named; the
10,000-seed sweep judges ~9.9M wire events with 0 violations and 0 silent
data corruptions in ~10 s; same-seed replay hashes are identical.

## Workspace layout

| Crate | Role | `no_std` |
|---|---|---|
| `crates/stablehal-contract` | Contract kernel: state machines, timing envelopes, clauses w/ spec citations, trace oracle, virtual time, seeded PRNG, trace hash | ✓ |
| `crates/stablehal-uart` | UART HAL: SW-facing `UartBus` trait + wire-level line contract (char timing, overlap, pairing) | ✓ |
| `crates/stablehal-sim` | Deterministic simulator + DST harness: virtual clock, envelope exploration, fault injection, online oracles, replay-by-seed | host |
| `crates/stablehal-modbus` | MODBUS RTU wire contract (12 clauses), conforming master driver, reference slave + bug catalog | ✓ |
| `crates/stablehal-conformance` | Conformance suite (cases `no_std`; simulator-hosted runner behind default `runner` feature) | core ✓ |
| `examples/modbus-master` | DST sweep demo / measurement source | |
| `examples/buggy-slave` | Conformance closed-loop demo | |
| `paper/` | Whitepaper draft with measured results | |

## Determinism guarantees

* Virtual time only — wall clock appears nowhere.
* One seeded SplitMix64 stream is the only entropy source (latencies inside
  contract envelopes, fault injection).
* Event-queue ties break by sequence number.
* Therefore: a run is a pure function of (config, seed, actors); every
  failure reproduces byte-for-byte from its seed (FNV-1a trace hash asserted
  in tests).

## Status / roadmap

* [x] M0 contract kernel + UART line contract
* [x] M1 deterministic simulator; planted-bug discovery + exact seed replay
* [x] M2 MODBUS RTU contract, master driver, reference slave, DST sweep
* [x] M3 conformance suite; closed loop proven positive (reference passes)
      and negative (6/6 bug catalog caught with exact clause)
* [x] M4 whitepaper draft with measurements
* [ ] Hardware phase (deferred; `no_std` cores need no rework): on-target
      runner (RP2040/STM32) + PIO bus observer, per-revision certification
* [ ] Second contract: I2C transaction layer (clock stretching, NACK,
      arbitration)
* [ ] Adapter for a third-party MODBUS crate behind `UartBus` — does the
      ecosystem actually conform?
