# stablehal-linux — host OS target

Linux counterpart to `targets/rp2040`: the unmodified reference drivers from
`stablehal-modbus`/`stablehal-spi`/`stablehal-i2c`/`stablehal-can`, running
against real Linux bus devices instead of the simulator's virtual wire, via
four `UartBus`/`SpiMaster`/`I2cMaster`/`CanController` implementations. No
board bring-up needed — UART and CAN are **verified working** (see below),
unlike `targets/rp2040` which is still drafted/unflashed (T9). SPI/I2C
compile clean but are **unverified against real hardware** (none on hand
this session) — treat those two like `targets/rp2040` until confirmed.

Standalone Cargo project (not a workspace member), matching `targets/rp2040`'s
precedent: needs `libc` for termios control, which has no place in the
`no_std`, zero-dep `crates/*` workspace (G1).

## Binaries

### UART / MODBUS RTU (`src/lib.rs`, verified)

* **`linux-dut-slave <device>`** — the unmodified `RtuSlave` reference
  driver, wired to a real serial fd. Conforming reference (`SlaveBugs::default()`).
* **`linux-tester <device> <capture-out>`** — drives one read-holding-registers
  transaction with the unmodified `RtuMaster` driver, records genuine
  per-byte `TxStart`/`TxEnd`/`Delivered` timing (not a skeleton trace — see
  `Rp2040`'s known gap), and writes a real SHC1 capture file.

### SPI (`src/spi.rs`, unverified)

* **`linux-spi-tester <spidev-path>`** — drives a scripted transaction with
  the unmodified `ScriptedSpiMaster` driver over `SpidevMaster`, a
  `spidev` port using the kernel's atomic `SPI_IOC_MESSAGE` ioctl (CS
  timing/per-byte edges aren't observable from userspace this way — see
  `spi.rs`'s module doc for the full tradeoff). Needs real hardware or a
  MOSI/MISO wire loopback.

### I2C (`src/i2c.rs`, unverified)

* **`linux-i2c-tester <i2c-bus-path> <addr7-hex>`** — drives a scripted
  write-then-read against the unmodified `ScriptedMaster` driver over
  `I2cDevMaster`, an `i2c-dev` port using the `I2C_RDWR` combined-transfer
  ioctl. Needs a real I2C peripheral at the given address.

### CAN (`src/can.rs`, verified)

* **`linux-can-loopback <interface>`** — transmits one frame and reads it
  back via `SocketCan`, a real non-blocking `AF_CAN`/`CAN_RAW` socket port
  (the one Linux bus port with a genuine async read/write primitive, no
  ioctl fakery needed). Self-contained on the kernel's virtual `vcan`
  interface:
  ```sh
  sudo modprobe vcan
  sudo ip link add dev vcan0 type vcan
  sudo ip link set up vcan0
  linux-can-loopback vcan0   # prints OK
  ```

## Verified loops (2026-09-14)

**UART / MODBUS RTU**, over a PTY pair
(`socat -d -d pty,raw,echo=0 pty,raw,echo=0`) — good for proving
protocol/framing correctness over real syscalls, though PTYs don't enforce
real bit-time pacing, so `UART-CHAR-TIME` timing isn't meaningfully
exercised this way:

```sh
cargo build
./target/debug/linux-dut-slave /dev/pts/8 &
./target/debug/linux-tester /dev/pts/9 capture.shc1
cargo run -p stablehal-certify -- judge capture.shc1 --timeout-ms 50
```

Result: `"verdict": "CONFORMING"`, 30 records, 0 gaps.

**CAN**, over `vcan0`:

```sh
sudo modprobe vcan && sudo ip link add dev vcan0 type vcan && sudo ip link set up vcan0
cargo build --bin linux-can-loopback
./target/debug/linux-can-loopback vcan0
```

Result: `OK` — frame sent and received back with matching id/payload.
First run failed (`timed out waiting for loopback frame`): a bare `AF_CAN`
socket does not receive frames it transmitted itself even on `vcan`
(`CAN_RAW_RECV_OWN_MSGS` is off by default, correctly — a real transceiver
doesn't hear its own TX as a separate RX event either). Fixed by adding a
`recv_own_msgs` parameter to `SocketCan::open`, on only for this
single-socket self-test; a real two-node setup (tester + DUT, separate
sockets) should leave it off.

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
* SPI/I2C: no SHC1 record encoding exists yet for either contract (only RTU
  and IPC are registered), so those two ports are functional-only — they
  prove the reference driver moves real bytes over a real bus, but produce
  no capture file for `stablehal-certify` to judge. Adding
  `CONTRACT_SPI_V0_1`/`CONTRACT_I2C_V0_1` + record kinds is separate,
  larger work (G5: contract-version-affecting) than porting the OS glue.
* SPI/I2C ioctls are atomic combined-transfer primitives — the kernel
  drives CS/START/STOP itself, so neither port can observe real per-byte
  or per-edge timing the way the UART and CAN ports can. See the tradeoff
  documented in each module's doc comment.
* CAN error-frame decoding is coarse: every kernel error frame currently
  maps to `CanError::BitError` rather than distinguishing stuff/form/ack/
  CRC errors (would need decoding `linux/can/error.h`'s data-byte layout).
