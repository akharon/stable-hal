# StableHAL — Next Steps (implementation handoff)

Audience: the next engineer or model continuing this project **from a cold
start**. Read order: [README.md](README.md) →
[docs/architecture.md](docs/architecture.md) →
[docs/design-spec-next.md](docs/design-spec-next.md) (the detailed design &
interface spec this backlog executes) → the user guides in [docs/](docs/) as
needed.

## Current state (verified 2026-07-07, updated 2026-07-08)

M0–M4 of the original plan are **complete**. T1–T6 and T8 from
next_steps.md are also **complete**; T7 is drafted but unverified:

| Task | Status | Notes |
|------|--------|-------|
| T1 — Repo hygiene | ✓ | `git init`, `v0.1.0` tag, `scripts/verify.sh`, clippy clean with ADR-9 allows |
| T2 — WP4 harness hardening | ✓ | `Report::probed_clauses()`, clause-coverage test, `SplitMix64::pick_biased()`, `SimConfig::boundary_bias`, `--bias` CLI flag |
| T3 — WP1 rmodbus adapter | ✓ | `adapters/rmodbus-slave/` with `RmodbusSlaveActor`, rmodbus `=0.12.2` pinned, **10/13 suite pass** (3 failures documented: larger register space, count=0 handling, no 128-reg cap) |
| T4 — WP2 step A | ✓ | `Scheduler<J>` extracted to `crates/stablehal-sim/src/sched.rs` |
| T5 — WP2 step B | ✓ | `crates/stablehal-i2c` contract (7 clauses) + `stablehal_sim::i2c::I2cWorld` + `stablehal_conformance::i2c` suite; closed loop + replay-exact |
| T6 — WP3.1+3.2 | ✓ | `crates/stablehal-shc1` (SHC1 codec) + `tools/stablehal-certify`; round-trip verdict identity proven for reference + all 6 SlaveBugs |
| T7 — WP3.3 | ~ | `targets/rp2040/` drafted (DUT + tester firmware, `UartBus` impl) but **unbuilt/unflashed/untested** — no `thumbv6m-none-eabi` target or hardware available this session. See its README for the itemized gap list (RX error decoding, TX/TxEnd timestamping in the tester). |
| T8 — Whitepaper v0.2 | ✓ | Folded in E5 (rmodbus), E6 (I2C), E7 (SHC1 round-trip); §6/§7 updated |

25 test binaries / all green across the workspace. `scripts/verify.sh`
exits 0 end to end (fmt, clippy -D warnings, tests, 10k-seed sweep,
replay-check, closed-loop demo) — a pre-existing T3 fmt/clippy drift in
`adapters/rmodbus-slave` was fixed along the way. All golden values intact,
including `modbus-master --replay-check 4242` = `95f0e863d22deaa9`.

## Environment notes

* Rust via rustup at `~/.cargo/bin` — **not on the default PATH**:
  `export PATH="$HOME/.cargo/bin:$PATH"` before any cargo command.
* The repo is a git repository with `v0.1.0` baseline tag applied.
* Toolchain used so far: rustc/cargo 1.96.1. No external crates anywhere in
  `crates/*` (binding rule G1 — see ground rules below). External deps in
  `adapters/rmodbus-slave/` only: `rmodbus =0.12.2`.

## Ground rules (from design-spec-next.md, binding)

* **G1** — zero external dependencies inside `crates/*`; third-party code
  only under `adapters/`, `targets/`, `tools/`.
* **G2** — determinism: no wall clock, single seeded PRNG per run, all
  randomness drawn in event order, total event order via `(at, seq)`.
* **G3** — every new clause: id + spec citation + one conformance case + one
  negative test proving the oracle catches it.
* **G4** — timing math only through shared `const fn`s.
* **G5** — public APIs documented; ChangeLog.txt updated per task; contract
  version bumps called out explicitly.
* Golden values that must survive refactors (assert, don't delete):
  replay hash `95f0e863d22deaa9` for `--replay-check 4242`; the 10k-sweep
  numbers; CRC vector `01 03 00 00 00 01 → 84 0A`.

## Ordered backlog

Tasks are ordered by (value ÷ risk); each is independently shippable. Spec
references are to [docs/design-spec-next.md](docs/design-spec-next.md).

### T1 — Repo hygiene (30 min) ✓
- [x] `git init`; commit baseline; tag `v0.1.0`.
- [x] Add `scripts/verify.sh` per **WP4 §4.3**; make it pass.
- [x] Silence the accepted clippy lints inline with
      `#[allow(clippy::large_enum_variant)]` + comment referencing ADR-9,
      then turn on `-D warnings` in verify.sh.

**Acceptance:** `scripts/verify.sh` exits 0 from a clean checkout.

### T2 — WP4 harness hardening (0.5 day) ✓
- [x] `Report::probed_clauses()` + suite-coverage test (**WP4 §4.1**).
- [x] `SplitMix64::pick_biased()` + `SimConfig::boundary_bias` flag,
      default `false` (**WP4 §4.2** — flipping the default breaks golden
      hashes; don't).

**Acceptance:** new tests green; existing golden values unchanged; a
10k sweep with `boundary_bias = true` (wire a `--bias` CLI flag through
`modbus-master`) still reports 0 violations / 0 property failures.

### T3 — WP1 rmodbus adapter (1 day) — *whitepaper headline E5* ✓
- [x] `adapters/rmodbus-slave/` per **WP1** (§1.2 layout, §1.3 attribution
      split, §1.4 poll behavior — mirror `RtuSlave` framing/pacing exactly).
- [x] Run `STANDARD_SUITE` against it; print the per-case table; pin the
      observed outcome as a golden assertion (§1.5).
- [ ] Record results + pinned rmodbus version in `paper/whitepaper.md` §5 as
      **E5**, and in ChangeLog.txt. *(whitepaper update deferred to T8)*

**Result:** rmodbus 0.12.2 passes **10/13** cases. 3 failures are documented
differences: larger register space (10k vs 128) means out-of-range addresses
return data instead of exception 02; count=0 on FC03 returns one register
instead of exception 03. These are valid measurements for the experiment.

### T4 — WP2 step A: Scheduler extraction (0.5–1 day, pure refactor) ✓
- [x] Extract `Scheduler<J>` (**WP2 §2.1**); rebuild `Sim::run_until` on it.
- [x] No public API change, no behavior change.

**Acceptance:** all tests green **and** `--replay-check 4242` prints
`95f0e863d22deaa9` (byte-identical traces prove the refactor is inert).

### T5 — WP2 step B: `stablehal-i2c` contract + `I2cWorld` (3–5 days) ✓
- [x] New crate `crates/stablehal-i2c` (`no_std`): `I2cTiming`,
      `I2cMaster`/`I2cOp`/`I2cFault`, `I2cSlaveDevice`, `I2cEvent`,
      `I2cContract` with the 7 v1 clauses (**WP2 §2.2.1–2.2.3**; UM10204
      citations; multi-master explicitly deferred).
- [x] Reference register-pointer device (24Cxx-style) + a conforming master
      driver (poll-driven, same conventions as `RtuMaster`).
- [x] `I2cWorld` in `stablehal-sim` on top of `Scheduler` (**§2.2.4**);
      per-byte SCL jitter and slave stretch drawn deterministically.
- [x] Negative catalog (`stretch_over_cap`, `nack_own_addr`,
      `wrong_read_data`) + DST test proving each is caught with the right
      clause; replay-exactness test for `I2cWorld`.
- [x] I2C conformance cases in `stablehal-conformance` (**§2.2.5**).
- [x] Respect the "what NOT to do" list (**§2.3**): `stablehal-i2c` depends
      only on `stablehal-contract`; `I2cWorld` builds its own `Scheduler`
      rather than reusing `Sim`; no merged event enum.

**Result:** `crates/stablehal-i2c/tests` (crate-level) + closed loop in
`crates/stablehal-sim/tests/i2c_dst.rs` (7 tests) +
`crates/stablehal-conformance/tests/i2c_closed_loop.rs` (2 tests), all
green. Reference master+device pass 50-seed envelope sweep; nack_own_addr
and stretch_over_cap each caught with the exact clause; wrong_read_data
confirmed as a non-breach (data bug, not protocol bug); seed 4242 replay
byte-exact. Docs: `docs/user-guide-i2c.md` + architecture.md §5.1 clause
registry; ChangeLog entry for contract `i2c v0.1`.

### T6 — WP3.1 + 3.2: capture format + certifier, sim-round-trip (1–2 days, no hardware) ✓
- [x] SHC1 format exactly per **WP3 §3.1** (incl. resync rule).
- [x] Sim-side SHC1 writer (serialize `sim.trace()`).
- [x] `tools/stablehal-certify` per **§3.2** (hand-rolled JSON, no deps).
- [x] Round-trip tests: conforming run and every `SlaveBugs` entry produce
      identical verdicts via sim oracle and via certifier-from-capture.

**Result:** new crate `crates/stablehal-shc1` (`Header`/`Record` codec,
`write_stream`, `parse_stream` with resync-and-count-gaps), new binary
`tools/stablehal-certify`. Round-trip test
(`crates/stablehal-shc1/tests/round_trip.rs`) confirms sim-oracle verdict ==
certifier-from-capture verdict for the conforming reference and all 6
`SlaveBugs` catalog entries. CLI smoke-tested end-to-end (`write_capture`
example → `stablehal-certify`), certificate JSON matches the spec schema
exactly (`stablehal-certificate-v1`, contract `modbus-rtu`/`0.1`/12
clauses). Certificate code `0x0001` (rtu v0.1) now live in the header; i2c
`0x0003` stays reserved (T5 did not touch the capture format).

### T7 — WP3.3: on-target tester (RP2040) (hardware required) — IN PROGRESS, code drafted / unverified
- [x] `targets/rp2040/` standalone crate (not a workspace member — needs
      `thumbv6m-none-eabi`, its own linker script/boot2, `rp2040-hal`):
      `Rp2040UartBus` (`UartBus` impl over the PL011 peripheral),
      `Clock` (1 MHz hardware timer -> `Nanos` bridge).
- [x] `dut-slave` binary: unmodified `RtuSlave` from `stablehal-modbus`
      driven over real UART0, conforming config (`SlaveBugs::default()`).
- [~] `tester` binary: unmodified `RtuMaster` drives one transaction over
      UART0; emits SHC1 header + `EndOfObservation` over UART1. **Not
      complete**: does not yet timestamp individual TxStart/TxEnd wire
      events into SHC1 records (needs either a PIO-based independent
      observer, or per-byte instrumentation of `Rp2040UartBus`) — without
      that, captures are framing-valid but empty of protocol content.
- [ ] Made `stablehal-shc1` `no_std` + `alloc` (was `std`-only) so the
      `Header`/`Record` codec layer is usable from the target crate; host
      `write_stream`/`parse_stream` (which need `Vec`) are unaffected,
      still exercised by the host test/tool suite.
- [ ] **Unverified**: no `thumbv6m-none-eabi` target installed and no
      RP Zero/RP2040 hardware available this session — code has not been
      compiled, flashed, or run. `rp2040_hal::uart::UartPeripheral` method
      names in `src/lib.rs` are written from memory of the 0.12 API and
      likely need fixing on first real build. See
      `targets/rp2040/README.md` "Status" section for the itemized gaps.
- [ ] Once hardware is available: install `thumbv6m-none-eabi` +
      `probe-rs-tools`, fix build errors, wire two RP Zeros per the
      README's bring-up plan, close the TX/TxEnd timestamping gap, and
      certify the reference DUT `CONFORMING`.

**Acceptance (not yet met):** a real capture file certified `CONFORMING` by
the unchanged host oracle; whitepaper §7 updated from "specified" to
"demonstrated".

### T8 — Whitepaper v0.2 (after T3, ideally after T5/T7) ✓
- [x] Fold in E5 (rmodbus), the I2C second-contract evidence (generalization
      claim). Hardware round-trip deferred (no T7/physical hardware this
      session) — §7 restructured to show E7 (SHC1 round-trip) done in
      simulation, items 2–3 (on-target runner, bus observer) still open.

**Result:** whitepaper draft v0.2. Abstract, §4.1 (new I2C pilot section),
§5 (E5/E6/E7 added, effort accounting updated), §6 (generalization limit
downgraded from "awaits a second contract" to "evidenced, two data
points"), §7 (restructured: capture/certify marked done, on-target runner
+ bus observer still open), §8 all updated. Every cited command
re-verified to reproduce exactly as written.

## Non-goals (do not start these)

Multi-master I2C arbitration, 10-bit addressing, SMBus PEC, MODBUS broadcast
/ extra function codes, TCP/OPC UA, edge-level electrical modeling, async
driver rewrites. Each would dilute the pilot before the loop is closed on
real hardware. Rationale: whitepaper §6, architecture.md risks T4/T5.

## Definition of done, per task

1. `scripts/verify.sh` passes (after T1 exists; until then: `cargo test` +
   both demo CLIs).
2. Golden values intact (or the change that legitimately alters them is
   called out in ChangeLog.txt with the new values).
3. ChangeLog.txt entry written; affected docs under `docs/` updated in the
   same change.
4. New behavior has both a positive test and a negative test (G3) — this
   project's credibility *is* its closed loop; keep it closed.
