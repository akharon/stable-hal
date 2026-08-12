# StableHAL — Next Steps (open backlog)

Read order for a cold start: [README.md](README.md) →
[docs/architecture.md](docs/architecture.md) → the user guides in
[docs/](docs/) as needed. Binding engineering ground rules (G1–G5) and the
golden values that must survive refactors live in **architecture.md §13**.

## Done (M0–M4 + T1–T14)

The contract kernel, deterministic simulator, MODBUS RTU / I2C / LIN / SPI /
CAN closed-loop pilots (all five now have a reference driver/device, a
`stablehal-sim` world, and a conformance suite), the IPC channel pilot (T14 —
the first non-wire domain: request/response datagrams over a bounded queue,
with scheduler-latency envelopes, backpressure, and peer death; closed loop
in **simulation only** — no capture/certify path exists for it, see
`docs/user-guide-ipc.md` §9), boundary-biased sampling,
the `rmodbus` third-party adapter (10/13 suite pass, documented differences),
the SHC1 capture format + `stablehal-certify` with round-trip verdict
identity, the envelope-containment pilot (MODBUS + I2C + LIN + SPI; T12 added
SPI), and whitepaper v0.2 are all complete and verified — `scripts/verify.sh`
exits 0 end to end. See `ChangeLog.txt` for the per-change history.

Ease of integration: the façade crate (`crates/stablehal`, one dependency +
one `prelude` covering every protocol) and a GitHub Actions CI job
(`.github/workflows/ci.yml` running `scripts/verify.sh` on every push) both
shipped. The one remaining idea — `crates.io` publishing — is deliberately
not started; it's a one-way public action that needs separate explicit
sign-off, not a code change.

Forward-looking design explorations that are **not** on this backlog (record/
replay statefulness limits, a declarative/typed contract vocabulary for LLM
authoring) live in [docs/design-notes.md](docs/design-notes.md).

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

### 2. IPC capture path (close the loop on a real socket)
The IPC pilot (T14) is closed-loop **in simulation only**. The oracle is
already `no_std` and judges any trace, but nothing produces a trace from a
real channel, so no running process can be certified. This is the IPC
analogue of item 1's gap, and it is cheaper: no hardware is needed.
- Emit `IpcEvent`s with timestamps from a real channel — an `LD_PRELOAD`
  interposer on `sendmsg`/`recvmsg`, or an eBPF probe. Lives outside
  `crates/*` (G1), under `tools/`.
- Assign an SHC1 contract code and extend `stablehal-certify` to judge IPC
  captures, reusing the existing `judge`/`measure`/`contain` subcommands.
- **Acceptance:** a capture from a real Unix-socket service certified, and
  its measured `IPC-RESP-TIME` hull compared against `sim_envelope_sweep`
  via the existing containment path. That comparison — CI's explored latency
  region vs. a loaded production box's actual one — is the concrete payoff,
  and the reason containment was built for this contract.

### 3. CAN bit-level arbitration contention
`CanWorld`'s multi-node topology now works (T13): a bus with several
contending testers arbitrates in `CanId`-priority order and terminates once
every driving node is done (`drives_termination()`), covered by
`tests/can_multinode.rs`. What remains a documented simplification is
*bit-level* arbitration — winners are picked by whole-frame `CanId` compare,
not modeled bit by bit, and same-id contention is resolved by a
deterministic registration-order tie-break rather than a real bus fault.
Revisit only if a pilot needs true bit-timing arbitration fidelity.

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
