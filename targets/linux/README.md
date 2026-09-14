# stablehal-linux — host OS target: UART over real termios

Linux counterpart to `targets/rp2040`: the unmodified `RtuMaster`/`RtuSlave`
drivers from `stablehal-modbus`, running against a real serial port instead
of the simulator's virtual wire, via a `UartBus` implementation over raw
termios (`SerialPort` in `src/lib.rs`). No board bring-up needed — this
target is **verified working** (see below), unlike `targets/rp2040` which is
still drafted/unflashed (T9).

Standalone Cargo project (not a workspace member), matching `targets/rp2040`'s
precedent: needs `libc` for termios control, which has no place in the
`no_std`, zero-dep `crates/*` workspace (G1).

## Binaries

* **`linux-dut-slave <device>`** — the unmodified `RtuSlave` reference
  driver, wired to a real serial fd. Conforming reference (`SlaveBugs::default()`).
* **`linux-tester <device> <capture-out>`** — drives one read-holding-registers
  transaction with the unmodified `RtuMaster` driver, records genuine
  per-byte `TxStart`/`TxEnd`/`Delivered` timing (not a skeleton trace — see
  `Rp2040`'s known gap), and writes a real SHC1 capture file.

## Verified loop (2026-09-14)

Run over a PTY pair (`socat -d -d pty,raw,echo=0 pty,raw,echo=0`) — good for
proving protocol/framing correctness over real syscalls, though PTYs don't
enforce real bit-time pacing, so `UART-CHAR-TIME` timing isn't meaningfully
exercised this way:

```sh
cargo build
./target/debug/linux-dut-slave /dev/pts/8 &
./target/debug/linux-tester /dev/pts/9 capture.shc1
cargo run -p stablehal-certify -- judge capture.shc1 --timeout-ms 50
```

Result: `"verdict": "CONFORMING"`, 30 records, 0 gaps.

## Real hardware

With two real USB-UART adapters (TX/RX crossed, common ground), the same
commands against `/dev/ttyUSB0` / `/dev/ttyUSB1` additionally exercise real
baud-rate timing — closing the hardware loop this project's oracle is
designed for, without waiting on `targets/rp2040` (T9)'s unresolved
`thumbv6m-none-eabi` bring-up.

## Known gaps

* Fixed termios baud table (`baud_constant`) covers common rates up to
  230400; arbitrary baud (`termios2`/`BOTHER`) not implemented.
* `UartError` decoding is coarse: any unexpected `read()` errno is reported
  as `Overrun` rather than distinguishing framing/parity/noise — real
  negative-path conformance cases (bad parity, framing errors) need a
  fault-injecting counterpart port or a signal-generator-driven capture to
  exercise meaningfully; termios on a clean USB-UART link rarely surfaces
  those errnos on its own.
* Single UART/MODBUS RTU only, matching the scoped first slice. SPI
  (`spidev`), I2C (`i2c-dev` ioctls), and CAN (`SocketCAN`) Linux ports are
  natural follow-ups using the same `SerialPort`-shaped pattern, once this
  one is exercised against real hardware.
