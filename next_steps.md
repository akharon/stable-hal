# StableHAL — Next Steps (open backlog)

Read order for a cold start: [README.md](README.md) →
[docs/architecture.md](docs/architecture.md) → the user guides in
[docs/](docs/) as needed. Binding engineering ground rules (G1–G5) and the
golden values that must survive refactors live in **architecture.md §13**.

## Done (M0–M4 + T1–T11)

The contract kernel, deterministic simulator, MODBUS RTU / I2C / LIN / SPI /
CAN closed-loop pilots (all five now have a reference driver/device, a
`stablehal-sim` world, and a conformance suite), boundary-biased sampling,
the `rmodbus` third-party adapter (10/13 suite pass, documented differences),
the SHC1 capture format + `stablehal-certify` with round-trip verdict
identity, the envelope-containment pilot (now MODBUS + I2C + LIN), and
whitepaper v0.2 are all complete and verified — `scripts/verify.sh` exits 0
end to end. See `ChangeLog.txt` for the per-change history.

Ease of integration: the façade crate (`crates/stablehal`, one dependency +
one `prelude`) and a GitHub Actions CI job (`.github/workflows/ci.yml`
running `scripts/verify.sh` on every push) both shipped. The one remaining
idea — `crates.io` publishing — is deliberately not started; it's a one-way
public action that needs separate explicit sign-off, not a code change.

## Open backlog

Ordered by (value ÷ risk). Each item is independently shippable. Cross-cutting
context is in architecture.md §11 (risks/debt).

### 1. On-target hardware certification (`targets/rp2040`) — the #1 credibility gap
The tester/DUT firmware is drafted but **never compiled, flashed, or run** (no
`thumbv6m-none-eabi` toolchain or hardware was available). Closes the
whitepaper's central claim.
- Install `thumbv6m-none-eabi` + `probe-rs-tools`; fix `rp2040-hal` 0.12 API
  calls written from memory in `targets/rp2040/src/lib.rs`.
- Close the TX/TxEnd timestamping gap so captures carry protocol content
  (per-byte instrumentation of `Rp2040UartBus`, or a PIO bus observer).
- Wire two RP2040/RP Zero boards per `targets/rp2040/README.md`; certify the
  reference DUT `CONFORMING` via the unchanged host oracle.
- **Acceptance:** a real capture certified `CONFORMING`; whitepaper §7 moves
  from "specified" to "demonstrated".

### 2. Extend envelope containment to SPI/CAN
`step_measured` is now implemented on `RtuContract`, `I2cContract`, and
`LinContract` (risk T11 closed for the protocols it was scoped to). SPI's
timing clauses (`SPI-XFER-TIME`, `SPI-CS-MIN-IDLE`) and CAN's frame-duration
estimate are candidates if containment claims are ever needed for those two
— not currently tracked as a numbered risk since the original risk register
only scoped T11 to I2C/LIN.

### 3. CAN arbitration/topology beyond the 2-node reference world
`CanWorld`'s arbitration (lowest `CanId` wins) and termination rule
(`nodes.iter().any(|n| n.done())`) are correct for the shipped reference
topology of one tester + one responder, but are explicitly documented
simplifications: real bit-level arbitration contention and a bus with more
than one actively-driving node aren't exercised. Revisit if a CAN pilot
needs a genuinely multi-node bus.

## Non-goals (do not start)

Multi-master I2C arbitration, 10-bit addressing, SMBus PEC, MODBUS broadcast /
extra function codes, TCP/OPC UA, edge-level electrical modeling, async driver
rewrites. Each dilutes the pilot before the loop is closed on real hardware
(whitepaper §6; architecture.md §3.2).

## Definition of done, per task

1. `scripts/verify.sh` passes.
2. Golden values (architecture.md §13.2) intact — or a legitimate change is
   called out in `ChangeLog.txt` with the new value.
3. `ChangeLog.txt` entry written; affected `docs/` updated in the same change.
4. New behavior has both a positive and a negative test (G3) — this project's
   credibility *is* its closed loop; keep it closed.
