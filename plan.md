# StableHAL — docs unification + usability/security + one capability — COMPLETE

Task: *"read docs, unify them, remove duplications; improve usability,
maintainability, security; make it extremely easy to start using and extremely
useful; adapt/improve interfaces and add features where needed."*

**Status: done. `./scripts/verify.sh` passes end to end; golden replay hash
`95f0e863d22deaa9` intact; 10k sweep 0 violations / 9,876,528 events; 13/13
reference + 6 bugs caught. Nothing committed.**

## Scope decisions (confirmed with user)
- market_analysis.md → **split**: design ideas kept, market sections dropped.
- code → **safe additive polish + one capability** (SPI envelope containment).

## What was delivered

### New capability — SPI envelope containment (T12)
- `SpiContract::step_measured` (measures `SPI-CS-MIN-IDLE` + `SPI-XFER-TIME`);
  `step` delegates to it (single judging path, ADR-11). Behaviour-neutral.
- `sim_envelope` in `stablehal_conformance::spi::runner`;
  new `tests/spi_envelope.rs` (2 tests, passing).
- CAN intentionally excluded (no timing-envelope clause) — documented, not forced.

### Usability
- Façade prelude (`crates/stablehal`) completed to cover **all** protocols
  (`i2c`/`lin`/`spi`/`can`/`shc1`) + sim-gated world types. Verified across the
  feature matrix incl. `--all-features` (no prelude name clashes).

### Maintainability / security / legal
- `LICENSE-MIT` + `LICENSE-APACHE` (Cargo.toml declared the dual license but no
  files existed), `CONTRIBUTING.md`, `SECURITY.md`, README "Contributing & license".
- Fixed ~7 pre-existing broken/private-item intra-doc links
  (embedded-hal, spi, can, sim) surfaced by the new doc gate.

### Docs unified / de-duplicated
- Stale fact fixed: `user-guide-lin.md` §8 (LIN suite *is* built).
- `architecture.md`: removed the duplicated clause registry (§5.1 → points to
  §13.3, the sole index); reconciled i2c cert-code label; swept §3.2/§5.6/§8.4/
  §9/§11 for T12.
- `market_analysis.md` → split into `docs/design-notes.md` (ideas) + deletion
  (market sections). `plan-usability.md` removed (all its work shipped).
- shc1 stale doc-comment fixed; README + next_steps cross-refs refreshed.

### CI hardening
- `scripts/verify.sh` (run by `ci.yml`) gained a façade feature-matrix check
  and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`.

### Changelog / backlog / memory
- `ChangeLog.txt`: 2026-08-11 T12 entry (no contract-version change; golden
  hash confirmed unchanged).
- `next_steps.md`: "extend containment to SPI/CAN" retired (SPI done, CAN N/A);
  design-notes linked.
- Project memory updated.

## Notes for a reviewer
- No MODBUS path touched → golden values unaffected (verified).
- No new external deps (G1), no new clauses / contract-version change (G3/G5).
- `SimConfig` builder methods already existed; all `crates/*` already
  `#![deny(unsafe_code)]` — no change needed there.

## Not done (out of chosen scope — see next_steps.md / docs/design-notes.md)
- On-target RP2040 hardware certification (the #1 open credibility gap).
- CAN multi-node topology.
- The larger declarative/typed-contract redesign (design-notes.md §2).
