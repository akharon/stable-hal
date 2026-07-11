# User Guide — I2C transaction contract (`stablehal-i2c`)

Audience: driver developers writing protocol code against the StableHAL I2C
boundary, and implementers providing an I2C backend (simulator or hardware).

v1 scope: single master, N slaves. Multi-master arbitration is reserved
(clause id `I2C-ARBITRATION`) and explicitly not implemented; 10-bit
addressing and SMBus PEC are also out of v1 scope (architecture.md §3.2).

## 1. The two faces of the crate

| Face | Type | Who uses it |
|---|---|---|
| SW-facing HAL | `I2cMaster` trait (master), `I2cSlaveDevice` trait (slave) | protocol drivers, device models |
| Wire-facing contract | `I2cContract` (implements `Contract`) | oracles, simulators, hardware observers |

Both are `no_std`. Modeled at **transaction/byte granularity** — bytes, ACK
bits, Start/Stop, stretch durations — not SDA/SCL edges. This is the
granularity software contracts actually live at (drivers see bytes and
ACKs); edge-level timing is electrical compliance, out of scope — the same
split as the UART line contract vs. physical layer.

## 2. Configuration

```rust
use stablehal_i2c::I2cTiming;

let timing = I2cTiming::standard(); // 100 kHz: scl 10..=100us, tBUF >= 4.7us
let timing = I2cTiming::fast();     // 400 kHz: scl 2.5..=25us, tBUF >= 1.3us
```

`stretch_max` defaults to 25 ms (SMBus 3.x clock-low-timeout convention) —
the I2C spec itself allows unbounded stretching; a deployment must bound it.

## 3. Writing a master driver against `I2cMaster`

```rust
use stablehal_i2c::{I2cMaster, I2cOp, I2cBusy};

fn issue_write<M: I2cMaster>(port: &mut M, addr7: u8) -> Result<(), I2cBusy> {
    port.try_begin(&I2cOp::Write { addr7, bytes: &[0x00, 0xAB], stop: true })
}

fn poll_completion<M: I2cMaster>(port: &mut M) {
    let mut buf = [0u8; 32];
    if let Some(result) = port.poll_done(&mut buf) {
        // Ok(()) — op completed (write acked, or read bytes are in `buf`).
        // Err(I2cFault::AddrNack) — nobody answered the address byte.
        // Err(I2cFault::DataNack{at_byte}) — slave stopped acking mid-write.
        // Err(I2cFault::StretchTimeout) — cumulative stretch exceeded the cap.
    }
}
```

`try_begin`/`poll_done` is single-op-at-a-time: `try_begin` returns
`Err(I2cBusy)` while a previous op is still in flight. `stop: false` leaves
the bus claimed for a repeated Start (e.g. write-then-read against a
register-pointer device without releasing the bus in between).

The reference driver, `stablehal_i2c::ScriptedMaster`, drives a fixed
`&'static [Step]` script one step at a time — used both as the DST reference
master and as the conformance-suite tester.

## 4. Implementing a slave device against `I2cSlaveDevice`

```rust
use stablehal_contract::Nanos;
use stablehal_i2c::I2cSlaveDevice;

trait MyDevice: I2cSlaveDevice {}
// on_start()/on_stop() mark transaction delimiters (reset any parse state);
// on_write(byte) -> (ack, stretch); on_read() -> (byte, stretch).
// ack_addr() defaults to true — override to plant a "nack my own address" bug.
```

The reference implementation, `RegisterPointerDevice`, is a 256-byte
24Cxx-EEPROM-style device: `Write [ptr, data...]` sets the pointer then
writes sequentially; `Read` returns bytes from the pointer, auto-
incrementing; the pointer wraps at 256. `RegisterPointerDevice::with_bugs`
plants exactly one fault from `I2cSlaveBugs` (`stretch_over_cap`,
`nack_own_addr`, `wrong_read_data`) — the negative catalog used by both the
DST closed loop and the conformance suite.

## 5. The I2C contract (what a party signs)

| Clause | Party | Obligation |
|---|---|---|
| `I2C-SEQ` | master | Well-formed sequencing: Start → AddrByte → DataBytes → (reStart \| Stop); no Stop without an open transaction; an open transaction at EndOfObservation is a breach. |
| `I2C-BUS-FREE` | master | ≥ `bus_free_min` idle time between a Stop and the next Start (UM10204 Table 10, tBUF). |
| `I2C-BYTE-TIME` | master | Each byte (9 SCL cycles) fits `9 × scl envelope` + observed stretch. |
| `I2C-READ-NACK-LAST` | master | The final byte of every Read is NACKed by the master; every earlier byte is ACKed. |
| `I2C-ADDR-ACK` | slave | A configured slave ACKs its own address byte. |
| `I2C-STRETCH-MAX` | slave | Cumulative clock stretch per byte ≤ `stretch_max`. |
| `I2C-WRITE-ACK` | slave | The reference register-pointer device ACKs all in-range writes. |

Judging a trace yourself:

```rust
use stablehal_contract::{Oracle, Nanos};
use stablehal_i2c::contract::{I2cContract, I2cEvent};
use stablehal_i2c::I2cTiming;

let mut oracle = Oracle::new(I2cContract::new(I2cTiming::standard(), &[0x50]));
oracle.observe(Nanos::ZERO, &I2cEvent::Start).unwrap();
match oracle.observe(Nanos::ZERO, &I2cEvent::AddrByte { addr7: 0x50, read: false, acked: true }) {
    Ok(()) => {}
    Err(v) => println!("{v}"), // names the clause, cites the spec, shows measured vs required
}
```

`I2cContract::new(timing, configured_addrs)` needs the configured address
set to distinguish an expected `AddrNack` (nobody home at that address) from
an `I2C-ADDR-ACK` breach (a configured device failed to answer its own
address).

Note: `wrong_read_data` (a device returning the wrong byte on read) is a
**data-correctness** bug, not a wire-contract breach — the oracle passes;
only a content-comparing conformance case (`I2C-WRITE-READBACK`) catches it.

## 6. Simulating a bus: `I2cWorld`

```rust
use stablehal_i2c::{I2cTiming, RegisterPointerDevice, ScriptedMaster, Step};
use stablehal_sim::i2c::{I2cMasterAdapter, I2cWorld, I2cWorldConfig};
use stablehal_contract::Nanos;

const SCRIPT: &[Step] = &[
    Step::Write { addr7: 0x50, bytes: &[0x00, 0xAB], stop: true },
];
let master = ScriptedMaster::new(SCRIPT);
let mut world = I2cWorld::new(
    I2cWorldConfig::new(/* seed */ 42, I2cTiming::standard()),
    Box::new(I2cMasterAdapter::new(master)),
);
world.add_slave(Box::new(RegisterPointerDevice::new(0x50)));
let summary = world.run_until(Nanos::from_millis(50)).unwrap();
println!("hash {:016x}", world.trace_hash());
```

`I2cWorld` mirrors `Sim`'s determinism guarantees (single seeded PRNG
stream, `(at, seq)` total event order via `Scheduler<Job>`) but is kept
independent per WP2 §2.3 — no merged event enum, no shared engine with the
UART `Sim`. Engine mechanics: each in-flight op executes byte by byte; per
byte the engine draws jitter for that byte's duration, asks the addressed
slave for ack/stretch, emits the wire events through the oracle, and
schedules the next byte at `now + byte_time (+ stretch)`. A Start is
deferred on the scheduler until `bus_free_min` has elapsed since the
previous Stop.

## 7. Conformance suite

`stablehal_conformance::i2c::STANDARD_SUITE` (behind the `runner` feature
for the simulator-hosted execution) exercises the reference DUT with five
cases: `I2C-WRITE-READBACK`, `I2C-POINTER-WRAP`, `I2C-ADDR-ACK`,
`I2C-OTHER-ADDR-SILENT`, `I2C-STRETCH-WITHIN-CAP`. Each case declares the
clause ids it probes; `Report::probed_clauses` supports the same coverage
accounting as the MODBUS suite (WP4 §4.1).
