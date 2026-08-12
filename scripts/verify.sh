#!/usr/bin/env bash
# StableHAL — CI gate / whitepaper reproduction script (WP4 §4.3).
# Exit nonzero on any failure.
set -euo pipefail

echo "=== 1. cargo fmt --check ==="
cargo fmt --check

echo "=== 2. cargo clippy --all-targets ==="
cargo clippy --all-targets -- -D warnings

echo "=== 3. cargo test ==="
cargo test

echo "=== 3b. facade feature matrix (no_std-clean + no prelude clashes) ==="
cargo check -p stablehal --no-default-features
cargo check -p stablehal --no-default-features --features "i2c lin spi can ipc shc1"
cargo check -p stablehal --all-features

echo "=== 3c. cargo doc (broken intra-doc links fail the build) ==="
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace

echo "=== 4. 10k-seed DST sweep ==="
cargo run --release -p modbus-master -- --seeds 10000

echo "=== 5. Replay-check (seed 4242) ==="
cargo run --release -p modbus-master -- --replay-check 4242

echo "=== 6. Closed-loop conformance demo ==="
cargo run --release -p buggy-slave

echo ""
echo "=== ALL CHECKS PASSED ==="
