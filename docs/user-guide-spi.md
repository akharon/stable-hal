# User Guide — SPI wire contract (`stablehal-spi`)

Audience: driver developers writing protocol code against the StableHAL SPI
boundary, and implementers providing an SPI backend (simulator or hardware).

v1 scope: single master, N chip-selected slaves on a shared bus. Multi-master
SPI is explicitly deferred (architecture.md §3.2).

## 1. The two faces of the crate

| Face | Type | Who uses it |
|---|---|---|
| SW-facing HAL | `SpiMaster` trait (master), `SpiSlave` trait (slave) | protocol drivers, device models |
| Wire-facing contract | `SpiContract` (implements `Contract`) | oracles, simulators, hardware observers |

Both are `no_std`. Modeled at **byte/word granularity with chip-select
edges** — not individual SCLK edges. SPI is fully synchronous and
full-duplex: every clock both transmits and receives a bit, so (unlike I2C)
there is no ack/stretch handshake — a device either responds correctly or it
doesn't; there's no wire-level "I refuse this byte" signal.

That symmetry has a consequence worth knowing before you plant a bug: CS
timing, mode discipline (CPOL/CPHA agreement), and transfer timing are
**master/world-level obligations**, not something a slave device can control
through `SpiSlave`'s hooks. `SpiSlaveBugs` therefore has exactly one field
(`wrong_read_data`, a data-correctness bug, not a wire breach) — the other
clauses are conforming by construction in this reference world and are
negative-tested directly against `SpiContract` via `Oracle`-driven unit
tests in `stablehal-spi/src/contract.rs`, not a device bug catalog.

## 2. Configuration

```rust
use stablehal_spi::SpiConfig;

let config = SpiConfig::default_1mhz(); // 1us SCLK period, 5% tolerance,
                                         // 100ns cs_idle_min, Mode0, MSB first
```

## 3. Writing a master driver against `SpiMaster`

```rust
use stablehal_spi::{SpiMaster, SpiBusy};

fn issue_transfer<M: SpiMaster>(port: &mut M, chip: u8, buf: &mut [u8]) -> Result<(), SpiBusy> {
    port.try_transfer(chip, buf)
}

fn poll_completion<M: SpiMaster>(port: &mut M) {
    if let Some(result) = port.poll_done() {
        // Ok(()) — transfer completed, `buf` (retained by the caller) holds RX bytes.
        // Err(SpiModeFault) — CS was lost mid-transfer.
    }
}
```

`try_transfer`/`poll_done` is single-op-at-a-time, same discipline as
`I2cMaster`. Because SPI is full-duplex, there's one call that both sends and
receives — `buf` is source (MOSI) going in and sink (MISO) once the transfer
completes; the caller must not read it before `poll_done()` confirms
completion (matching real async/DMA driver discipline).

The reference driver, `stablehal_spi::ScriptedSpiMaster`, drives a fixed
`&'static [SpiStep]` script one step at a time — used both as the DST
reference master and as the conformance-suite tester. Its own recorded
`SpiStepOutcome::Transferred` RX buffer is not populated by `SpiWorld` (the
world doesn't need to route completed data back through the caller-owned
buffer for the reference master's own logic — it's script-driven, not
data-driven — mirroring `stablehal_lin::master::SlotOutcome`'s discarded
buffer for the same reason); wire-level correctness is judged from the
emitted `ByteXfer` trace events, not this convenience buffer.

## 4. Implementing a slave device against `SpiSlave`

```rust
use stablehal_spi::{SpiSlave, SpiMode};

trait MyDevice: SpiSlave {}
// on_select(mode) — CS asserted, prepare for a transfer.
// xfer_byte(mosi) -> miso — one byte exchanged, synchronously.
// on_deselect() — CS de-asserted, transaction over.
```

The reference implementation, `SpiRegisterDevice`, is a 256-byte
register-pointer device (the same first-byte-is-pointer idiom as
`stablehal_i2c::RegisterPointerDevice`): the first byte of a transfer sets
the pointer; subsequent bytes write-then-auto-increment-read.
`SpiRegisterDevice::with_bugs` plants `SpiSlaveBugs::wrong_read_data`.

## 5. The SPI contract (what a party signs)

| Clause | Party | Obligation |
|---|---|---|
| `SPI-CS-TIMING` | master | CS must be asserted for the duration of a transfer and de-asserted between transfers; no overlapping CS assertions. |
| `SPI-MODE-DISC` | master | Master and slave must agree on CPOL/CPHA for the transaction. |
| `SPI-XFER-TIME` | master | Each byte transfer fits within 8× the SCLK period envelope (plus tolerance). |
| `SPI-DATA-VALID` | — | Master/slave provide valid data on MISO/MOSI during the active clock phase; no bit-level collisions. **Vestigial**: declared but never checked in `step` — no event at this granularity carries bit-collision information. Documented, not silently dropped (same honesty pattern as CAN's `CAN-BIT-STUFFING`). |
| `SPI-MODE-FAULT` | slave | A slave must not drive MISO unless its CS is asserted. Unreachable from a conforming reference master/device in this world's topology; not closed-loop tested. |
| `SPI-TRANSACTION` | master | Every byte belongs to exactly one transaction (between CS assert and CS de-assert). |
| `SPI-CS-MIN-IDLE` | master | Minimum idle time between CS de-assert of one transfer and CS assert of the next. |

`SpiContract`'s initial state is `Idle { since: Nanos::ZERO }` — not an
`Option`, unlike I2C's `BusPhase::Free { since: Option<Nanos> }` — so the
very first CsAssert of *any* observation is judged as if idle since time
zero: it must still wait out `cs_idle_min`. `SpiWorld` accounts for this by
treating "no prior CsDeAssert" the same as "deasserted at time zero," not as
"no gate at all" — worth knowing if you drive `SpiContract` directly from
your own harness.

Judging a trace yourself:

```rust
use stablehal_contract::{Oracle, Nanos};
use stablehal_spi::contract::{SpiContract, SpiEvent};
use stablehal_spi::{SpiConfig, SpiMode};

let mut oracle = Oracle::new(SpiContract::new(SpiConfig::default_1mhz()));
oracle.observe(Nanos::from_micros(10), &SpiEvent::CsAssert { chip: 0, mode: SpiMode::Mode0 }).unwrap();
```

## 6. Simulating a bus: `SpiWorld`

```rust
use stablehal_spi::{ScriptedSpiMaster, SpiConfig, SpiRegisterDevice, SpiStep};
use stablehal_sim::spi::{SpiMasterAdapter, SpiWorld, SpiWorldConfig};
use stablehal_contract::Nanos;

const SCRIPT: &[SpiStep] = &[
    SpiStep { chip: 0, tx: &[0x00, 0xAB] },
];
let master = ScriptedSpiMaster::new(SCRIPT);
let mut world = SpiWorld::new(
    SpiWorldConfig::new(/* seed */ 42, SpiConfig::default_1mhz()),
    Box::new(SpiMasterAdapter::new(master)),
);
world.add_slave(0, Box::new(SpiRegisterDevice::new(0)));
let summary = world.run_until(Nanos::from_millis(50)).unwrap();
println!("hash {:016x}", world.trace_hash());
```

`SpiWorld` mirrors `I2cWorld`'s shape (1 master, N chip-selected slaves,
`Scheduler<Job>`-driven) but is simpler: SPI has no address phase and no
ack/stretch, so each in-flight transfer is just CsAssert (once), then N
byte-timed `ByteXfer` ticks, then CsDeAssert — no separate Addressing/Write/
Read split.

## 7. Conformance suite

`stablehal_conformance::spi::STANDARD_SUITE` (behind the `runner` feature)
exercises the reference DUT with three cases: `SPI-BASIC-TRANSFER`,
`SPI-MULTI-BYTE-TRANSFER`, `SPI-BACK-TO-BACK-TRANSFERS`, covering
`SPI-TRANSACTION`, `SPI-CS-TIMING`, `SPI-MODE-DISC`, `SPI-XFER-TIME`, and
`SPI-CS-MIN-IDLE`. `SPI-MODE-FAULT` and `SPI-DATA-VALID` are excluded from
the suite's coverage assertion — see §5 for why.

## 8. Envelope containment

Since T12, `SpiContract` implements `Contract::step_measured` (ADR-11): its
two timing clauses `SPI-CS-MIN-IDLE` and `SPI-XFER-TIME` report every
measured duration — passing or not — through a side channel, so the
sim-explored hull can be compared against a hardware-measured one.
`stablehal_conformance::spi::runner::sim_envelope(seed)` drives the reference
master/device over a back-to-back-transfer script and folds those
measurements into an `EnvelopeAccumulator`; `tests/spi_envelope.rs` asserts a
nonempty hull per clause on a conforming run. This is the same mechanism used
for MODBUS/I2C/LIN — see [user-guide-containment.md](user-guide-containment.md)
for the full model. (The `stablehal-certify` CLI's containment subcommands
remain MODBUS-only; SPI containment is exercised through the conformance
runner + tests.)
