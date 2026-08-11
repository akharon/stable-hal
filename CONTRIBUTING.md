# Contributing to StableHAL

Thanks for your interest. StableHAL's credibility *is* its closed loop — a
deterministic contract judging both simulation and hardware — so the
contribution rules exist to keep that loop closed and reproducible. Read this
once; it is short.

## Ground rules (binding — see [docs/architecture.md](docs/architecture.md) §13)

| Id | Rule |
|----|------|
| **G1** | `crates/*` carry **zero external dependencies**. Third-party code and MCU HALs live only under `adapters/`, `targets/`, `tools/`. The one exception is `stablehal-embedded-hal` (its whole purpose is bridging to `embedded-hal`). |
| **G2** | **Determinism**: no wall clock; one seeded `SplitMix64` per run; every random draw happens in event order; total event order is `(at, seq)`. |
| **G3** | Every new clause carries `id` + spec `citation` + one-sentence `text`, and is exercised by **at least one conformance case and one negative test** that proves the oracle catches its violation. |
| **G4** | Timing derivations go through shared `const fn`s (`UartConfig::char_time`, `Nanos::mul_ratio`, the `*Timing` constructors) — never duplicated arithmetic. |
| **G5** | Public API additions get doc comments; a contract-version-affecting change gets an explicit `ChangeLog.txt` entry. |

## Golden values (never silently change)

These are asserted in tests and pinned in architecture.md §13.2. A change that
legitimately moves one must call it out in `ChangeLog.txt` **and** update the
value there in the same commit:

- MODBUS replay hash `95f0e863d22deaa9` (`modbus-master --replay-check 4242`)
- 10k-seed sweep: 0 violations, 9,876,528 events (`modbus-master --seeds 10000`)
- CRC-16/MODBUS vector `01 03 00 00 00 01` → `84 0A`
- `SplitMix64` seed 0, first output `0xe220a8397b1dcdaf`

## Definition of done (per change)

1. `./scripts/verify.sh` passes end to end (fmt, clippy `-D warnings`, tests,
   10k sweep, replay check, closed-loop demo).
2. Golden values intact — or the change is documented as above.
3. `ChangeLog.txt` entry written; affected `docs/` updated in the **same** change.
4. New behavior has both a positive and a negative test (G3).

## Toolchain

Stable Rust ≥ 1.96 (`rust-version` in the workspace `Cargo.toml`). No network
fetch is needed for anything under `crates/`, `examples/`, or `tools/`; only
`adapters/rmodbus-slave` pulls a third-party crate.

## Where to start

- New to the codebase? Read [README.md](README.md) →
  [docs/getting-started.md](docs/getting-started.md) → the protocol guide you
  need → [docs/architecture.md](docs/architecture.md) for the *why*.
- Open work is tracked in [next_steps.md](next_steps.md); forward-looking design
  explorations that are not yet scheduled live in
  [docs/design-notes.md](docs/design-notes.md).

## Licensing of contributions

By contributing you agree that your contributions are licensed under the same
terms as the project: **MIT OR Apache-2.0** (see [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE)).
