# Usability Work Package — Plan & Detailed Design

Status: **proposed** (research + design complete, implementation not started).
Constraint: **strictly additive** — no change to existing behavior, public API
signatures, trace hashes, golden values (`95f0e863d22deaa9`), contract
semantics, or test outcomes. Anything that would move a golden value is out of
scope by definition.

## 1. Context

StableHAL's engineering core is in good shape (strong crate-level rustdoc,
seven reference guides, a reproducible `scripts/verify.sh` gate), but the
*first hour* of a new user is unnecessarily hard. Research findings (two
codebase surveys, 2026-07-10):

1. **No getting-started path.** The docs are per-protocol *reference* guides
   plus a 44 KB `architecture.md`; the vocabulary (contract / oracle / actor /
   envelope) is only defined in the big doc. There is no
   "install → run → write your first actor" tutorial. Canonical usage lives in
   tests (`crates/stablehal-sim/tests/dst.rs`,
   `crates/stablehal-conformance/tests/closed_loop.rs`), where newcomers don't
   look.
2. **README opens at maximum abstraction.** The value proposition in plain
   task terms ("test your MODBUS driver with no hardware, reproduce every
   failure from a seed") is not the first thing a reader sees. The quickstart
   runs the maintainers' verification sweep, not a user-shaped hello world.
3. **No façade crate.** Integration means wiring 4–5 path deps
   (`-contract`, `-uart`, `-sim`, `-modbus`, `-conformance`) and knowing how
   they compose. `next_steps.md` item 5 already proposes a `stablehal`
   re-export crate — unbuilt.
4. **Toolchain requirement (1.96+) is prose-only.** No `rust-version` in
   `[workspace.package]`, no `rust-toolchain.toml`; older toolchains fail with
   raw compile errors instead of a clear message.
5. **CLIs have no `--help`.** `modbus-master` silently ignores unknown flags;
   `buggy-slave` takes no args; `stablehal-certify` prints usage only when the
   subcommand is missing. Flags (`--seeds`, `--corrupt`, `--replay-check`,
   `--bias`) are discoverable only by reading source.
6. **No CI.** `scripts/verify.sh` is manual; no green-build signal, and the
   good rustdoc is unpublished (nothing tells the user to `cargo doc --open`).
7. **Config ergonomics.** `SimConfig::new(seed, uart)` forces
   mutate-after-new for `latency`/`corrupt_ppm`/`boundary_bias`; conformance
   preconditions (`DUT_UNIT = 0x11`, registers pre-set to
   `pattern(addr) = 0xA000 | addr`) are prose-only magic.

Intended outcome: a newcomer goes from `git clone` to *their own actor running
under DST with a contract observer* in under 15 minutes, using one dependency
and one tutorial, with everything existing untouched.

## 2. Work items (tiered)

### Tier 1 — docs & metadata only (no code compiled differently)

#### T1.1 `docs/getting-started.md` (new)

A single linear tutorial. All snippets must compile against the real APIs
(verified against `crates/stablehal-sim/src/lib.rs` and
`crates/stablehal-sim/tests/dst.rs`). Outline:

1. **What you'll do** — 3 sentences: run the demos, write a 60-line simulated
   device pair, watch a contract catch a planted bug, replay it from its seed.
2. **Prerequisites** — stable Rust ≥ 1.96; no other tools; no network fetch
   needed except the `adapters/rmodbus-slave` experiment.
3. **Clone and smoke-test** — `cargo test`; what "green" looks like.
4. **Run the demos** — `cargo run --release -p buggy-slave` (13/13 pass, six
   buggy slaves each named with the breached clause) and
   `cargo run --release -p modbus-master -- --seeds 1000`; explain the output
   lines and the meaning of the trace hash.
5. **Hello, simulator** — the heart of the tutorial. Walks through
   `examples/hello-sim` (T2.2) section by section: the `Actor` trait
   (`fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll`),
   the `NextPoll::{At, OnRx, Done}` scheduling contract, `SimConfig::new`,
   `Sim::new(cfg, Box::new(a0), Box::new(a1))`, `run_until`, `trace_hash()`.
6. **Make a contract watch the wire** — add a one-clause `Contract`
   (mirroring `GapContract` in `tests/dst.rs:37-80`: `Clause` static,
   `initial`/`step`/`clauses`) and attach it with
   `sim.add_observer(Box::new(ContractObserver::new(contract, map_fn, None)))`.
   Plant the bug, sweep seeds, replay the failing seed, show identical hash.
7. **Certify an implementation** — the `run_suite(&SuiteConfig::default_9600(),
   STANDARD_SUITE, factory)` snippet from `docs/user-guide-conformance.md`,
   plus the two preconditions stated explicitly: DUT unit `0x11`, holding
   registers pre-initialized to `pattern(addr) = 0xA000 | addr`.
8. **Where to go next** — reading order: this tutorial → protocol user guide →
   `architecture.md` for the why; plus `cargo doc --open -p stablehal-sim`.

#### T1.2 README restructure (reorder + additions; delete nothing)

Keep every existing sentence, table, command, and golden hash. New order:

1. Title + tagline (unchanged).
2. **New: "What is this for?"** — 3 plain-language sentences: *test protocol
   drivers with zero hardware; every run is a pure function of a seed, so
   every failure replays byte-for-byte; certify real hardware against the
   same oracle that judged the simulation.*
3. **New: "Start here"** — one line linking `docs/getting-started.md` and
   `cargo run --release -p hello-sim`.
4. Quick start (unchanged, incl. expected output and `95f0e863d22deaa9`).
5. The idea in one paragraph (unchanged, moved below Quick start).
6. Integrating your own driver (unchanged table).
7. Documentation table — add a `getting-started.md` first row, add a line
   noting API docs are available via `cargo doc --open`, and add the
   recommended reading order (tutorial → guides → architecture).
8. Workspace layout / Determinism guarantees / Status (unchanged; add one
   row for `crates/stablehal` and `examples/hello-sim` when T2 lands).

#### T1.3 Toolchain floor: `rust-version`

- Root `Cargo.toml` `[workspace.package]`: add `rust-version = "1.96"`.
- Each member `Cargo.toml`: add `rust-version.workspace = true`.
- **Recommendation: do NOT add `rust-toolchain.toml`.** Pinning a channel
  forces toolchain downloads and fights users' rustup defaults; the MSRV
  field is enough to turn a cryptic build failure into
  `package requires rustc 1.96 or newer`.
- Safety: pure metadata; cannot change codegen, behavior, or hashes.

### Tier 2 — additive code

#### T2.1 Façade crate `crates/stablehal` (new workspace member)

One `cargo add`-shaped dependency and one prelude. Only workspace path deps,
so rule G1 (zero *external* deps under `crates/*`) is preserved.

`crates/stablehal/Cargo.toml`:

```toml
[package]
name = "stablehal"
description = "Facade: one dependency + one prelude for the StableHAL workspace"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[features]
default = ["sim", "modbus", "conformance"]   # the batteries-included host set
sim = ["dep:stablehal-sim"]                  # host-only (std)
modbus = ["dep:stablehal-modbus"]
i2c = ["dep:stablehal-i2c"]
lin = ["dep:stablehal-lin"]
spi = ["dep:stablehal-spi"]
can = ["dep:stablehal-can"]
shc1 = ["dep:stablehal-shc1"]
conformance = ["sim", "dep:stablehal-conformance"]

[dependencies]
stablehal-contract = { workspace = true }    # always: the kernel, no_std
stablehal-uart = { workspace = true }        # always: the pilot HAL, no_std
stablehal-sim = { workspace = true, optional = true }
stablehal-modbus = { workspace = true, optional = true }
stablehal-i2c = { workspace = true, optional = true }
stablehal-lin = { workspace = true, optional = true }
stablehal-spi = { workspace = true, optional = true }
stablehal-can = { workspace = true, optional = true }
stablehal-shc1 = { workspace = true, optional = true }
stablehal-conformance = { workspace = true, optional = true }
```

`crates/stablehal/src/lib.rs` — re-exports only, no logic:

```rust
//! One-stop façade. `use stablehal::prelude::*;` and go.
//! Embedded/`no_std` consumers: `default-features = false` + protocol features.
#![cfg_attr(not(feature = "sim"), no_std)]

pub use stablehal_contract as contract;
pub use stablehal_uart as uart;
#[cfg(feature = "sim")] pub use stablehal_sim as sim;
#[cfg(feature = "modbus")] pub use stablehal_modbus as modbus;
#[cfg(feature = "i2c")] pub use stablehal_i2c as i2c;
#[cfg(feature = "lin")] pub use stablehal_lin as lin;
#[cfg(feature = "spi")] pub use stablehal_spi as spi;
#[cfg(feature = "can")] pub use stablehal_can as can;
#[cfg(feature = "shc1")] pub use stablehal_shc1 as shc1;
#[cfg(feature = "conformance")] pub use stablehal_conformance as conformance;

pub mod prelude {
    pub use stablehal_contract::{
        Breach, Clause, Contract, Nanos, Oracle, SplitMix64, TimeBound, Violation,
    };
    pub use stablehal_uart::{TxBusy, UartBus, UartConfig, UartError};
    #[cfg(feature = "sim")]
    pub use stablehal_sim::{
        Actor, BusObserver, ContractObserver, NextPoll, Sim, SimConfig, UartPort,
    };
    #[cfg(feature = "modbus")]
    pub use stablehal_modbus::{
        MasterConfig, RegisterBank, RtuContract, RtuMaster, RtuSlave, RtuTiming,
        SlaveConfig,
    };
    #[cfg(feature = "conformance")]
    pub use stablehal_conformance::{
        pattern, runner::run_suite, runner::SuiteConfig, DUT_UNIT, STANDARD_SUITE,
    };
}
```

Not re-exported deliberately: `stablehal-embedded-hal` (it carries external
deps; users who want the bridge add it directly, keeping the façade G1-clean).
`stablehal-embedded-hal` remains outside so that `crates/stablehal` itself has
zero external transitive deps.

Register in root `Cargo.toml`: `members += ["crates/stablehal"]`,
`[workspace.dependencies] stablehal = { path = "crates/stablehal" }`.

#### T2.2 `examples/hello-sim` (new workspace member)

~80 heavily commented lines; the artifact the tutorial (T1.1 §5–6) walks
through, and a compile-time smoke test for the façade. Depends only on
`stablehal` (features `sim`). Shape (condensed from
`crates/stablehal-sim/tests/dst.rs`):

```rust
//! Hello, StableHAL: two toy devices, one contract, one seed.
use stablehal::prelude::*;

const UART: UartConfig = UartConfig::rtu_default(9600);

struct Greeter { sent: bool }          // device 0: sends one byte
impl Actor for Greeter {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        /* try_write(0x48) once, then NextPoll::Done */
    }
}
struct Echoer;                          // device 1: acks every byte
impl Actor for Echoer { /* try_read loop, try_write(0xEE), NextPoll::OnRx */ }

fn main() {
    let mut sim = Sim::new(
        SimConfig::new(42, UART),
        Box::new(Greeter { sent: false }),
        Box::new(Echoer),
    );
    let summary = sim.run_until(Nanos::from_millis(10)).unwrap();
    println!("events={} hash={:016x}", summary.events, sim.trace_hash());
    // Same seed ⇒ same hash: rerun and assert equality to demo determinism.
}
```

Runs as `cargo run -p hello-sim`; prints event count + trace hash + a replay
equality line. Exit 0.

#### T2.3 CLI `--help`

All three binaries; additive checks before existing parsing so current flag
behavior is untouched.

- `examples/modbus-master/src/main.rs` — at top of `main()`:
  `if args.iter().any(|a| a == "--help" || a == "-h") { print usage; return; }`
  Help text: the four flags with defaults (`--seeds N` [1000],
  `--corrupt PPM` [2000], `--replay-check SEED`, `--bias`), one line each,
  plus the exit-code meaning.
- `examples/buggy-slave/src/main.rs` — same pattern; text says it takes no
  arguments and describes the closed-loop demo.
- `tools/stablehal-certify/src/main.rs` — add one match arm **before** the
  flag catch-all: `"--help" | "-h" => { usage(); ExitCode::SUCCESS }`.
  (Today `--help` falls into the `_` arm → usage + FAILURE; the only visible
  change is exit code 0 for an explicit help request — additive intent, no
  existing scripted path relies on `--help` failing.)

#### T2.4 Doctests / doc examples

- `crates/stablehal-sim/src/lib.rs` crate docs: append a minimal runnable
  doctest (two inline actors + `run_until` + `trace_hash`), marked normal
  (host crate — doctests fine).
- `crates/stablehal-contract/src/oracle.rs` on `Oracle`: a doctest driving a
  one-clause contract through `observe`. (no_std lib, but doctests build as
  host binaries — fine.)
- Doc comments only; zero effect on compiled library code.

### Tier 3 — polish

#### T3.1 CI: `.github/workflows/ci.yml`

```yaml
name: ci
on: [push, pull_request]
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: ./scripts/verify.sh
```

Plus a CI badge line at the top of README once the repo has a GitHub remote.

#### T3.2 `SimConfig` builder-style methods (additive)

In `crates/stablehal-sim/src/lib.rs`, next to `SimConfig::new` (keep `new`
and public fields exactly as-is):

```rust
impl SimConfig {
    pub fn with_latency(mut self, latency: TimeBound) -> Self { self.latency = latency; self }
    pub fn with_corrupt_ppm(mut self, ppm: u32) -> Self { self.corrupt_ppm = ppm; self }
    pub fn with_boundary_bias(mut self, bias: bool) -> Self { self.boundary_bias = bias; self }
}
```

Pure additions; existing mutate-after-new call sites keep compiling untouched.

#### T3.3 Document the conformance magic constants where users hit them

Doc-comment additions only: on `DUT_UNIT` and `pattern` in
`crates/stablehal-conformance/src/lib.rs`, state the precondition sentence
("the DUT must answer unit 0x11 with holding registers pre-set to
`pattern(addr)`, or every case fails") and cross-link
`docs/user-guide-conformance.md`.

## 3. Implementation order

1. T1.3 MSRV metadata (smallest, unblocks a clean error for everyone else).
2. T2.1 façade crate → T2.2 hello-sim (tutorial depends on both existing).
3. T1.1 getting-started tutorial → T1.2 README restructure (links to all of it).
4. T2.3 `--help`, T2.4 doctests.
5. T3.1 CI, T3.2 builders, T3.3 constant docs.

## 4. Verification (prove nothing changed, prove the new works)

**Regression gate — run before starting and after every tier:**

- `./scripts/verify.sh` fully green (fmt, clippy `-D warnings`, tests,
  10k-seed sweep, replay check, closed-loop demo).
- `cargo run --release -p modbus-master -- --replay-check 4242` still prints
  the golden hash `95f0e863d22deaa9`.
- `cargo test 2>&1 | grep 'test result'` — identical pass counts before/after.

**New-surface checks:**

- `cargo check -p stablehal` (defaults), `--no-default-features`,
  `--no-default-features --features modbus`, `--features conformance` —
  feature matrix compiles; the `--no-default-features` build proves the
  façade stays `no_std`-clean.
- `cargo run -p hello-sim` — exits 0, prints a stable hash across two runs.
- `--help` on all three binaries: correct text, exit 0, and
  `modbus-master --seeds 10` still behaves identically (help path not taken).
- `cargo test --doc -p stablehal-sim -p stablehal-contract` — new doctests pass.
- Tutorial snippets: every code block in `docs/getting-started.md` must be a
  verbatim excerpt of `examples/hello-sim` or an existing test, so the
  example's compilation *is* the doc's verification.

## 5. Explicit non-goals / guardrails

- No change to `SplitMix64`, `Scheduler`, event ordering, or anything hash-
  adjacent. Never flip `boundary_bias` defaults.
- No signature changes to `SimConfig::new`, `Sim::new`, `Actor`, `Contract`,
  `run_suite`; no field removals or renames anywhere.
- No new external dependencies under `crates/*` (G1); no clap — help stays
  hand-rolled like the rest of the CLI layer.
- No crates.io publishing in this package (names/versions stay local); the
  façade is designed so publishing later is possible without rework.
- rmodbus stays pinned at `=0.12.2`; adapter untouched.
