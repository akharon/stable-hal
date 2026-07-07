# StableHAL — Next Steps (implementation handoff)

Audience: the next engineer or model continuing this project **from a cold
start**. Read order: [README.md](README.md) →
[docs/architecture.md](docs/architecture.md) →
[docs/design-spec-next.md](docs/design-spec-next.md) (the detailed design &
interface spec this backlog executes) → the user guides in [docs/](docs/) as
needed.

## Current state (verified 2026-07-07)

M0–M4 of the original plan are **complete**: contract kernel, UART line
contract, deterministic simulator, 12-clause MODBUS RTU contract with
conforming master/slave, 13-case conformance suite, closed loop proven in
both directions, whitepaper draft with measurements. 24 tests green.
`cargo test` and the demo CLIs are the source of truth.

## Environment notes

* Rust via rustup at `~/.cargo/bin` — **not on the default PATH**:
  `export PATH="$HOME/.cargo/bin:$PATH"` before any cargo command.
* The repo is **not a git repository yet**. Recommended first action:
  `git init`, commit everything as `v0.1.0` baseline, then one commit per
  task below.
* Toolchain used so far: rustc/cargo 1.96.1. No external crates anywhere in
  `crates/*` (binding rule G1 — see ground rules below).

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

### T1 — Repo hygiene (30 min)
- [ ] `git init`; commit baseline; tag `v0.1.0`.
- [ ] Add `scripts/verify.sh` per **WP4 §4.3**; make it pass.
- [ ] Optional: silence the accepted clippy lints inline with
      `#[allow(clippy::large_enum_variant)]` + comment referencing ADR-9,
      then turn on `-D warnings` in verify.sh.

**Acceptance:** `scripts/verify.sh` exits 0 from a clean checkout.

### T2 — WP4 harness hardening (0.5 day)
- [ ] `Report::probed_clauses()` + suite-coverage test (**WP4 §4.1**).
- [ ] `SplitMix64::pick_biased()` + `SimConfig::boundary_bias` flag,
      default `false` (**WP4 §4.2** — flipping the default breaks golden
      hashes; don't).

**Acceptance:** new tests green; existing golden values unchanged; a
10k sweep with `boundary_bias = true` (wire a `--bias` CLI flag through
`modbus-master`) still reports 0 violations / 0 property failures.

### T3 — WP1 rmodbus adapter (1 day) — *whitepaper headline E5*
- [ ] `adapters/rmodbus-slave/` per **WP1** (§1.2 layout, §1.3 attribution
      split, §1.4 poll behavior — mirror `RtuSlave` framing/pacing exactly).
- [ ] Run `STANDARD_SUITE` against it; print the per-case table; pin the
      observed outcome as a golden assertion (§1.5).
- [ ] Record results + pinned rmodbus version in `paper/whitepaper.md` §5 as
      **E5**, and in ChangeLog.txt.

**Acceptance:** deterministic, pinned suite outcome; whitepaper updated.
Pass *or* fail of rmodbus is equally acceptable — it is the measurement.

### T4 — WP2 step A: Scheduler extraction (0.5–1 day, pure refactor)
- [ ] Extract `Scheduler<J>` (**WP2 §2.1**); rebuild `Sim::run_until` on it.
- [ ] No public API change, no behavior change.

**Acceptance:** all tests green **and** `--replay-check 4242` prints
`95f0e863d22deaa9` (byte-identical traces prove the refactor is inert).

### T5 — WP2 step B: `stablehal-i2c` contract + `I2cWorld` (3–5 days)
- [ ] New crate `crates/stablehal-i2c` (`no_std`): `I2cTiming`,
      `I2cMaster`/`I2cOp`/`I2cFault`, `I2cSlaveDevice`, `I2cEvent`,
      `I2cContract` with the 7 v1 clauses (**WP2 §2.2.1–2.2.3**; UM10204
      citations; multi-master explicitly deferred).
- [ ] Reference register-pointer device (24Cxx-style) + a conforming master
      driver (poll-driven, same conventions as `RtuMaster`).
- [ ] `I2cWorld` in `stablehal-sim` on top of `Scheduler` (**§2.2.4**);
      per-byte SCL jitter and slave stretch drawn deterministically.
- [ ] Negative catalog (`stretch_over_cap`, `nack_own_addr`,
      `wrong_read_data`) + DST test proving each is caught with the right
      clause; replay-exactness test for `I2cWorld`.
- [ ] I2C conformance cases in `stablehal-conformance` (**§2.2.5**).
- [ ] Respect the "what NOT to do" list (**§2.3**).

**Acceptance:** mirror of the MODBUS closed-loop test set, for I2C:
reference passes suite; every bug caught with exact clause; seed replay
exact; docs: short `docs/user-guide-i2c.md` + clause table added to
architecture.md §5/§6; ChangeLog entry introducing contract `i2c v0.1`.

### T6 — WP3.1 + 3.2: capture format + certifier, sim-round-trip (1–2 days, no hardware)
- [ ] SHC1 format exactly per **WP3 §3.1** (incl. resync rule).
- [ ] Sim-side SHC1 writer (serialize `sim.trace()`).
- [ ] `tools/stablehal-certify` per **§3.2** (hand-rolled JSON, no deps).
- [ ] Round-trip tests: conforming run and every `SlaveBugs` entry produce
      identical verdicts via sim oracle and via certifier-from-capture.

**Acceptance:** round-trip identity holds; certificate JSON matches the
schema in the spec; format registry table updated.

### T7 — WP3.3: on-target tester (RP2040) (hardware required)
- [ ] First: refactor `TesterActor`/`StepPlan` into a `no_std` module of
      `stablehal-conformance` (**§3.3**, keeps sim runner working — golden
      values again).
- [ ] `targets/rp2040-tester`: pinned `rp-hal`, 1 MHz timebase, `UartBus`
      impl, SHC1 over USB-CDC, self_report flag.
- [ ] Certify the reference: RP2040 tester ↔ second board running... the
      simplest DUT is another RP2040 flashed with `RtuSlave` behind the same
      `UartBus` impl. Then a third-party hardware MODBUS device.

**Acceptance:** a real capture file certified `CONFORMING` by the unchanged
host oracle; whitepaper §7 updated from "specified" to "demonstrated".

### T8 — Whitepaper v0.2 (after T3, ideally after T5/T7)
- [ ] Fold in E5 (rmodbus), the I2C second-contract evidence (generalization
      claim), and hardware round-trip if available; refresh the limits
      section; version the contracts in Table 1.

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
