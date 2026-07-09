# User Guide — LIN transaction contract (`stablehal-lin`)

Audience: driver developers writing protocol code against the StableHAL LIN
boundary, and implementers providing a LIN backend (simulator or hardware).

v1 scope: single master, TDMA schedule table, enhanced checksum only
(classic checksum — data bytes without the PID — is a future revision if a
target deployment needs it).

## 1. LIN is structurally different from MODBUS and I2C

Worth naming up front, because it shapes the API:

* **No arbitration** (unlike I2C) — LIN has exactly one master; there is
  never a multi-party race for the bus.
* **No request/response coupling** (unlike MODBUS) — the master's only
  recurring obligation is to emit a conforming header (break, break
  delimiter, sync, PID) *on schedule*. Whether anyone answers, and whether
  that answer is correct, is a separate obligation belonging to whichever
  slave publishes that frame ID (or no one, for an unassigned ID).

This is why `LinContract` has two clauses with no MODBUS/I2C analogue:
`LIN-SLAVE-SILENCE` (an assigned publisher must not go silent) and
`LIN-ONE-RESPONSE` (nothing may respond out of turn).

## 2. The two faces of the crate

| Face | Type | Who uses it |
|---|---|---|
| SW-facing HAL | `LinBus` trait (master), `LinSlaveDevice` trait (slave) | protocol drivers, device models |
| Wire-facing contract | `LinContract` (implements `Contract`) | oracles, simulators, hardware observers |

Both are `no_std`.

## 3. Configuration

```rust
use stablehal_lin::LinTiming;

let timing = LinTiming::new(19_200); // common LIN bus speed
```

Every timing bound (`break_min`, `break_delimiter_min`, `byte_time`,
`header_nominal`, `response_nominal`, `frame_max`, `interbyte_max`) derives
compositionally from the single nominal `bit_time = 1/baud`, per LIN 2.2A
§2.3.1.3 — no separately-tuned constants, so simulator, oracle, and any
target implementation compute identical bounds from the same baud rate.

## 4. Writing a master driver against `LinBus`

```rust
use stablehal_lin::{LinBus, ScheduleEntry, ScheduleMaster};

const SCHEDULE: &[ScheduleEntry] = &[
    ScheduleEntry { id: 0x10, n_data: 2 },
    ScheduleEntry { id: 0x21, n_data: 4 },
];

fn drive<B: LinBus>(bus: &mut B) {
    let mut master = ScheduleMaster::new(SCHEDULE);
    // call master.step(bus) from your poll loop until master.done()
}
```

`try_send_header`/`poll_response` is single-slot-at-a-time: `try_send_header`
returns `Err(LinBusy)` while a previous slot's response window hasn't
closed. The reference driver, `stablehal_lin::ScheduleMaster`, drives a
fixed `&'static [ScheduleEntry]` schedule table one slot at a time — used
both as the DST reference master and as a conformance-suite tester.

## 5. Implementing a slave device against `LinSlaveDevice`

```rust
use stablehal_lin::LinSlaveDevice;

trait MyNode: LinSlaveDevice {}
// respond(pid, id) -> Option<([u8; 8], len, checksum)>:
//   Some(...) if this device publishes a response for `id`;
//   None to stay silent (either "not mine" or, for bug injection,
//   "mine but deliberately withheld").
```

The reference implementation, `TableSlave`, is a fixed signal table: each
`PublishedFrame { id, data, len }` is one frame this device answers;
`set_data` lets the application update published values (e.g. a sensor
reading) between schedule cycles. `TableSlave::with_bugs` plants exactly
one fault from `LinSlaveBugs` (`corrupt_checksum`, `respond_to_unowned`,
`silent_on_owned`) — the negative catalog used by the DST closed loop.

## 6. The LIN contract (what a party signs)

| Clause | Party | Obligation |
|---|---|---|
| `LIN-BREAK` | master | The break field is driven low for >= 13 nominal bit times at the start of every frame. |
| `LIN-BREAK-DELIM` | master | The break delimiter (recessive) lasts >= 1 nominal bit time between break and sync. |
| `LIN-SYNC` | master | The sync byte is exactly 0x55. |
| `LIN-PID-PARITY` | master | The PID's two parity bits are computed correctly from the 6-bit frame ID. |
| `LIN-FRAME-MAXLEN` | master | The complete frame fits within `T_frame_max = 1.4 x T_frame_nominal`. |
| `LIN-INTERBYTE-MAX` | slave | No inter-byte gap in the response exceeds the per-byte tolerance budget. |
| `LIN-CHECKSUM` | slave | The response checksum matches the data (enhanced: PID + data, inverted mod-256 sum). |
| `LIN-SLAVE-SILENCE` | slave | A slave assigned to publish this frame ID never withholds its response. |
| `LIN-ONE-RESPONSE` | slave | At most one response is observed per frame slot. |

Judging a trace yourself:

```rust
use stablehal_contract::{oracle::check_trace, Nanos};
use stablehal_lin::contract::{LinContract, LinEvent, enhanced_checksum, encode_pid};
use stablehal_lin::LinTiming;

let timing = LinTiming::new(19_200);
let contract = LinContract::new(timing, &[(0x10, 2)]); // publisher: id 0x10, 2 data bytes
let pid = encode_pid(0x10);
let data = [0xAAu8, 0xBB];
let checksum = enhanced_checksum(pid, &data);
// build a TraceEvent<LinEvent> sequence: Break, BreakDelimiter, Sync,
// Pid, DataByte x2, Checksum, EndOfObservation — see
// crates/stablehal-lin/src/contract.rs tests for a full worked example.
```

`LinContract::new(timing, publishers)` needs the `(frame_id, n_data)`
publisher list to size the frame timing budget per ID and to judge
`LIN-SLAVE-SILENCE` — an unassigned ID going unanswered is legal;
an assigned publisher going silent is a breach.

## 7. Simulating a bus: `LinWorld`

```rust
use stablehal_lin::device::{PublishedFrame, TableSlave};
use stablehal_lin::master::{ScheduleEntry, ScheduleMaster};
use stablehal_lin::timing::LinTiming;
use stablehal_sim::lin::{LinMasterAdapter, LinWorld, LinWorldConfig};
use stablehal_contract::Nanos;

const SCHEDULE: &[ScheduleEntry] = &[ScheduleEntry { id: 0x10, n_data: 2 }];
let master = ScheduleMaster::new(SCHEDULE);
let mut world = LinWorld::new(
    LinWorldConfig::new(/* seed */ 42, LinTiming::new(19_200)),
    Box::new(LinMasterAdapter::new(master)),
);
let frame = PublishedFrame { id: 0x10, data: [0xAA, 0xBB, 0, 0, 0, 0, 0, 0], len: 2 };
world.add_slave(Box::new(TableSlave::new(&[frame])), &[(0x10, 2)]);
let summary = world.run_until(Nanos::from_millis(50)).unwrap();
println!("hash {:016x}", world.trace_hash());
```

`LinWorld` mirrors `Sim`'s and `I2cWorld`'s determinism guarantees (single
seeded PRNG stream, `(at, seq)`-ordered event queue via `Scheduler<Job>`)
but is kept independent per WP2 §2.3 — no merged event enum, no shared
engine. Engine mechanics: each scheduled slot runs as one unit — emit the
header (break/delimiter/sync/PID) with jittered timing, then, if a
registered slave owns the frame ID, emit its response bytes + checksum with
jittered inter-byte gaps; otherwise emit `NoResponse`.

**Jitter caution baked into the simulator, worth knowing if you extend
it:** break and break-delimiter timing are *floor-only* obligations (no
contractual maximum) — the engine draws their jitter with
`jitter_at_least(min)` (never below the floor) rather than the symmetric
`jitter(nominal)` used for byte/inter-byte gaps (which have real ceilings).
Using symmetric jitter on a floor-only bound would make the reference
master occasionally breach its own contract — this was caught by the DST
closed-loop test during development and is exactly the kind of bug the
closed loop exists to catch.

## 8. Conformance suite

Not yet built — LIN v1 currently ships the contract, reference master/slave,
and simulator world with a closed-loop DST test
(`crates/stablehal-sim/tests/lin_dst.rs`), but no
`stablehal_conformance::lin` case suite analogous to MODBUS's
`STANDARD_SUITE` or I2C's suite yet. See `next_steps.md` for backlog status.
