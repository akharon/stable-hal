# stablehal-rp2040 — T7 on-target conformance runner (WP3.3)

Bare-metal RP2040 firmware for closing the hardware loop described in
`docs/architecture.md` §7 / `paper/whitepaper.md` §7. Two binaries:

* **`dut-slave`** — the unmodified `RtuSlave` driver from `stablehal-modbus`,
  configured as the conforming reference (`SlaveBugs::default()`), running
  against a real UART instead of the simulator.
* **`tester`** — drives one transaction with the unmodified `RtuMaster`
  driver, streams an SHC1 capture header + `EndOfObservation` over a second
  UART for the host to save and feed to `stablehal-certify`.

This is a standalone Cargo project (not a workspace member of the main
repo) because it targets `thumbv6m-none-eabi`, needs its own linker script
(`memory.x`) and boot2 stage, and pulls in `cortex-m`/`rp2040-hal` that have
no place in the host `std` workspace.

## Status (as of this commit)

Builds are **not yet verified** — `thumbv6m-none-eabi` is not installed in
this environment and no RP2040/RP Zero hardware was on hand this session.
Code review before flashing:

* `src/lib.rs` — `Rp2040UartBus`: `UartBus` adapter over the PL011
  peripheral. **Known gap:** the RX error path (`service()`) does not yet
  decode PL011 framing/parity/overrun status bits into `UartError` — it
  currently only distinguishes "byte available" vs "nothing yet". Fix
  before trusting negative-path (`UartError`) conformance cases on target.
* `src/bin/dut_slave.rs` — should be flashable as-is; only wires the real
  UART0 to `RtuSlave::poll`, no protocol logic of its own.
* `src/bin/tester.rs` — drives one read-holding-registers transaction and
  emits a *skeleton* SHC1 stream (header + `EndOfObservation` only).
  **Known gap:** it does not yet timestamp individual `TxStart`/`TxEnd`
  wire events into SHC1 records — that needs either a PIO program watching
  the DUT's TX line independently (true third-party observer, sets
  `FLAG_OBSERVER`) or per-byte instrumentation of `Rp2040UartBus`'s
  `try_write`/`service` (self-report, `FLAG_SELF_REPORT` — what the header
  currently declares). Without real TX/TxEnd records, `stablehal-certify`
  will only ever see a trivially-empty conforming trace; this is the next
  concrete step once boards are on the bench.

## Bring-up plan (RP Zero × 2)

1. Install the target and a flashing tool:
   ```sh
   rustup target add thumbv6m-none-eabi
   cargo install probe-rs-tools   # or elf2uf2-rs for drag-and-drop UF2 flashing
   ```
2. Wire board A (DUT) UART0 (GP0 TX / GP1 RX) to board B (tester) UART0
   (GP1 RX / GP0 TX — crossed), plus a common ground.
3. Wire board B's UART1 (GP4 TX / GP5 RX) to a USB-serial adapter or the
   host, for the SHC1 capture stream.
4. Flash `dut-slave` to board A, `tester` to board B.
5. Capture the UART1 output to a file (`capture.shc1`).
6. `cargo run -p stablehal-certify -- capture.shc1 --timeout-ms 50` from
   the host workspace root — must report `CONFORMING` once the TX/TxEnd
   timestamping gap above is closed.

## Design notes

* Timing: `Clock::now()` reads the RP2040's free-running 1 MHz timer via
  `rp2040_hal::Timer` and converts ticks to `Nanos::from_micros`, so the
  same `poll(now, bus)` driver code runs unmodified from simulation to
  target — the whitepaper's central claim.
* No allocator: `stablehal-shc1` is `no_std` + `alloc`-free at its
  `Header`/`Record` encode/decode layer (fixed-size byte arrays); this
  target only uses that layer, never `write_stream`/`parse_stream` (which
  need `alloc::vec::Vec` and are host-only).
