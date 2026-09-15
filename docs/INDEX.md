# StableHAL Documentation Index

Complete navigation guide for StableHAL's deterministic protocol testing framework.

## Essential Reading

1. **[getting-started.md](getting-started.md)** — 15-minute quickstart
2. **[architecture.md](architecture.md)** — Full Arc42 spec, design decisions, glossary
3. **Protocol guides** — Pick your protocol:
   - [user-guide-uart.md](user-guide-uart.md) — Character timing, line contract
   - [user-guide-modbus.md](user-guide-modbus.md) — 12-clause RTU wire contract
   - [user-guide-i2c.md](user-guide-i2c.md) — 7-clause I2C addressing
   - [user-guide-lin.md](user-guide-lin.md) — 9-clause LIN schedule tables
   - [user-guide-spi.md](user-guide-spi.md) — 7-clause SPI timing
   - [user-guide-can.md](user-guide-can.md) — 9-clause CAN 2.0B broadcast
   - [user-guide-ipc.md](user-guide-ipc.md) — 9-clause IPC channels

4. **Advanced topics:**
   - [user-guide-simulator.md](user-guide-simulator.md) — DST loop, actors, sweeps
   - [user-guide-conformance.md](user-guide-conformance.md) — Test suite structure
   - [user-guide-containment.md](user-guide-containment.md) — Envelope proof method
   - [design-notes.md](design-notes.md) — Unscheduled design explorations

## Key Concepts

| Concept | Reference |
|---------|-----------|
| Ground Rules (G1-G5) | architecture.md § 13.1 |
| Golden Values | architecture.md § 13.2 |
| Contract Kernel | architecture.md § 5.2 |
| ADRs (1-11) | architecture.md § 9 |
| Clause Registry | architecture.md § 5.1 |
| Testing Strategy | architecture.md § 8.4 |

## Commands

```bash
cargo test                          # Full verification
./scripts/verify.sh                 # CI gate (fmt, clippy, tests, sweep)
cargo run --release -p buggy-slave  # Closed-loop demo
cargo run --release -p modbus-master -- --seeds 10000  # DST sweep
```
