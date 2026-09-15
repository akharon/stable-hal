# Configuration Reference

Quick guide to all configuration files in StableHAL.

---

## Configuration Files

| File | Purpose | Key Settings | Notes |
|------|---------|--------------|-------|
| `Cargo.toml` | Rust project manifest | dependencies, features, workspace | Crate versions, test config |
| `Cargo.lock` | Locked dependency versions | All pinned versions | Commit for reproducible builds |
| `.cargo/config.toml` | Cargo build configuration | build settings, optimization levels | Per-workspace profile config |
| `rust-toolchain.toml` | Rust version constraint | channel (stable/nightly) | Ensures toolchain consistency |
| `.rustfmt.toml` | Code formatter rules | max_width, edition | Code style enforcement |
| `.clippy.toml` | Linter configuration | allow, deny, warn rules | Static analysis strictness |
| `CONTRIBUTING.md` | Developer guidelines | Build rules, test requirements, definitions of done | See [CONTRIBUTING.md](../CONTRIBUTING.md) |

---

## Critical Configuration Points

### No External Dependencies (G1 Rule)
- **`Cargo.toml` (all `crates/*`)**: Zero external deps except `stablehal-embedded-hal`
- **Rationale**: Determinism + security; embedded systems can't use unstable libraries
- **Checked by**: CI gate runs `cargo tree --all-features` — must show no external deps

### Rust Version
- **`rust-toolchain.toml`**: Specifies exact Rust version (e.g., `1.75`)
- **Why exact**: Newer versions may change code generation; need reproducible behavior
- **To update**: Edit file, run `rustup update`, commit new hash

### Protocol Timing Constants (G4 Rule)
- **Location**: `crates/*/src/constants.rs`
- **Examples**: `const TIMEOUT_MS: u32 = 200;` — never hardcode timeouts elsewhere
- **Compliance**: All tests and production code reference these constants
- **To add timing**: Add to `constants.rs`, reference everywhere

### Test Gate (CI)
- **`./scripts/verify.sh`**: Runs full CI pipeline
- **Stages**: Build → Format check → Lint → Test → Doc build
- **Must pass before merge**

---

## How to Modify Common Configurations

### Update Rust Version
```toml
# Edit rust-toolchain.toml
[toolchain]
channel = "1.76"  # Was 1.75

# Then locally:
rustup update
cargo build

# Commit the updated file
```

### Add Protocol Timing Constant (G4 Rule)
```rust
// Edit crates/i2c/src/constants.rs
pub const I2C_CLOCK_CYCLE_US: u32 = 10;  // 100 kHz
pub const I2C_TIMEOUT_MS: u32 = 50;

// Use everywhere (never hardcode):
// WRONG: let timeout = Duration::from_millis(50);
// RIGHT:
let timeout = Duration::from_millis(I2C_TIMEOUT_MS as u64);
```

### Add New Protocol Conformance Clause (G3 Rule)
```rust
// In crates/modbus/src/lib.rs
/// Positive case: valid function code
#[test]
fn test_function_code_01_valid() {
  // Test conformance
}

/// Negative case: invalid function code
#[test]
fn test_function_code_invalid_rejects() {
  // Test rejection
}
```

### Run Full Verification Locally
```bash
# Runs all CI checks
./scripts/verify.sh

# Expected output: All checks green ✓
# If any fail: Fix before commit
```

---

## Build Profiles

**Development** (default):
```bash
cargo build  # Debug symbols, slow but catches errors early
```

**Release** (optimized):
```bash
cargo build --release  # Optimized, no debug info
```

**Testing**:
```bash
cargo test  # Runs all unit tests
./scripts/verify.sh  # Runs CI gate + extra checks
```

---

## Code Style & Linting

### Format Code
```bash
cargo fmt  # Auto-format all code (non-negotiable)
# Configured by .rustfmt.toml (max_width: 100, edition: 2021)
```

### Lint Code
```bash
cargo clippy  # Run linter
# Config in .clippy.toml; most warnings are errors (strict mode)
```

### Pre-Commit Check
```bash
# Before committing, run:
cargo fmt --all
cargo clippy --all-targets
```

---

**Last updated:** 2026-09-15
