# StableHAL — Codebase Guide

## Session preferences

- Keep chat responses silent/minimal to save tokens — avoid narration, progress updates, or verbose explanations unless asked.
- Use `/pairs` skill for code navigation instead of Read/grep — saves 40-60% of lookup tokens.
- Batch independent Read/Edit/Bash calls together to reduce round-trips.
- Trust diffs on edits — never ask to verify changes after Edit tool succeeds.
- Ask for specific line ranges when reading large files, not whole files.
- Reference code by `file:line` markdown links instead of quoting content.
- **Keep test/diagnostic output token-optimized** — run tests/scripts in quiet or summary mode, avoid verbose/debug logging flags, and don't dump full logs into chat; report pass/fail counts and only the relevant failure excerpt.
- **Save project context to memory** → `/home/mkyr/.claude/projects/-home-mkyr/memory/` (e.g., architectural decisions, common patterns, token optimizations learned).

---

## Quick Summary

**StableHAL** — Rust framework for deterministic, contract-enforcing embedded protocol testing. Seeded simulation + hardware capture judged against same oracle. Five protocols (MODBUS, I2C, LIN, SPI, CAN) + IPC. `crates/*` = no_std, zero external deps. DST (Deterministic Simulation Testing) proves simulation exercises real hardware envelope.

---

## Reading Order

1. **[docs/CONFIG.md](docs/CONFIG.md)** — Configuration files reference (Cargo.toml, rust-toolchain, timing constants, etc.)
2. **[docs/DATA.md](docs/DATA.md)** — Test fixtures, golden values, conformance cases, protocol specs
3. **[docs/INDEX.md](docs/INDEX.md)** — Quick navigation by use case
4. **[docs/getting-started.md](docs/getting-started.md)** — 15-minute intro
5. **[docs/architecture.md](docs/architecture.md)** — Full Arc42 spec, ADRs, golden values
6. **Protocol guide** — Pick your protocol from [docs/INDEX.md](docs/INDEX.md)

---

## Golden Comments (Ground Rules G1-G5)

- **G1: `crates/*` zero external deps** — One exception: `stablehal-embedded-hal`. All others (MODBUS, I2C, LIN, SPI, CAN) fully self-contained.
- **G2: Determinism via seeded PRNG** — No wall clock, no thread-local state. Seed determines all randomness. Simulation and hardware must use same seed.
- **G3: Every new clause: positive + negative** — Contract additions require conformance case + violation case. Both pass on real hardware.
- **G4: Timing through shared `const fn`s** — Never hardcode timeouts. Define in `constants.rs` per protocol; all clauses reference same source.
- **G5: Doc comments + `ChangeLog.txt`** — Any contract-affecting change documented inline + entry in `ChangeLog.txt` with date/author.

See [docs/architecture.md](docs/architecture.md) § 13 for golden values and details.

---

## Scripts & Automation

Available in `/scripts/`:
- `verify.sh` — CI gate (build, test, lint, docs). Fastest validation.
- `test-hardware.sh` — Run tests against real hardware (requires device setup).
- `generate-golden.sh` — Capture golden values from hardware for new protocol clause.
- `bench.sh` — Benchmark simulation vs. hardware timing.

**Bash aliases** (add to session):
```bash
alias test='cargo test'
alias verify='./scripts/verify.sh'
alias test-hw='./scripts/test-hardware.sh'
alias gen-golden='./scripts/generate-golden.sh'
```

---

**Key Files:**
- [docs/INDEX.md](docs/INDEX.md) — Topic index
- [CONTRIBUTING.md](CONTRIBUTING.md) — Bound rules, definitions of done
- [README.md](README.md) — Project overview

## PAiRS Skill Shortcuts

Use [/pairs skill](/.claude/skills/pairs) for code navigation.

**Common queries for StableHAL:**
```bash
# Find all protocol implementations (MODBUS, I2C, LIN, SPI, CAN)
/pairs find_symbol "Protocol" --regex --preview

# Verify determinism: find all PRNG seeding
/pairs grep_symbols "seed|Prng" --regex --preview

# Check golden value contract compliance
/pairs find_symbol "golden_.*" --regex

# Trace conformance case coverage (G3 rule)
/pairs impact ContractClause --depth 2

# Find all timing-related constants (G4 rule)
/pairs grep_symbols "const.*timeout|const.*delay" --regex
```

**Commands:**
```bash
cargo test                                    # Full verification
./scripts/verify.sh                           # CI gate
cargo run --release -p buggy-slave            # Demo
```

---

**ADR Reference:** View architectural decisions:
```bash
git log --grep="ADR" --oneline
```

---

## Maintenance

**When adding/removing config files:**
1. Update [docs/CONFIG.md](docs/CONFIG.md) — add/remove file row
2. Document purpose and how to modify it

**When adding/removing test fixtures or protocol conformance cases:**
1. Update [docs/DATA.md](docs/DATA.md) — add/remove file row
2. Add golden values for new protocol clauses (positive + negative cases, G3 rule)
3. Document how to capture hardware responses and add new conformance cases

**Example:** If you add LIN protocol support:
```markdown
| File | Purpose | Format | Usage |
|------|---------|--------|-------|
| `crates/lin/tests/fixtures/golden_lin_*.json` | LIN command/response golden values | JSON | Conformance tests |
```

**Last updated:** 2026-09-15 (Token-optimized with memory enforcement)
