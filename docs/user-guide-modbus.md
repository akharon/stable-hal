# User Guide — MODBUS RTU Stack (`stablehal-modbus`)

Audience: users of the master driver / reference slave, and anyone reading or
extending the RTU wire contract.

Subset (fixed for v0.1): FC 0x03 Read Holding Registers, 0x06 Write Single
Register, 0x10 Write Multiple Registers, exception responses; unicast only.

## 1. Timing setup

```rust
use stablehal_contract::Nanos;
use stablehal_modbus::RtuTiming;
use stablehal_uart::UartConfig;

let uart = UartConfig::rtu_default(9600);              // 8E1, 11-bit chars
let timing = RtuTiming::from_uart(&uart, Nanos::from_millis(50));
// timing.t15 = 1.718749 ms, timing.t35 = 4.010415 ms at 9600 baud
// above 19200 baud the spec fixes t15 = 750 µs, t35 = 1.75 ms
```

`response_timeout` (here 50 ms) is a **contract term**: the slave must *start*
its response within `[t3.5, response_timeout]` after the request's last
character, and the master must not retry before it expires. Use one
`RtuTiming` value everywhere (driver, slave, oracle, conformance) — mixed
timeouts across components are a spec violation waiting to happen.

## 2. Master driver (`RtuMaster`)

Poll-driven, `no_std`, written only against `UartBus`:

```rust
use stablehal_modbus::{MasterConfig, MasterProgress, RtuMaster};
use stablehal_modbus::pdu::Request;

let mut master = RtuMaster::new(MasterConfig::new(timing)); // retries = 2

// Begin a transaction (fails with `false` while one is in flight):
master.start(0x11, &Request::ReadHoldingRegisters { addr: 0, count: 8 });

// Drive it from your loop / sim actor / ISR tick:
match master.poll(now, &mut bus) {
    MasterProgress::Busy { wake_at } => { /* schedule next poll; RX also wakes you */ }
    MasterProgress::Done(Ok(adu)) => {
        use stablehal_modbus::pdu::{parse_response, ParsedResponse};
        let r = parse_response(0x11, 0x03, adu.bytes()).unwrap();
        let v0 = r.register(0); // Option<u16>
    }
    MasterProgress::Done(Err(e)) => { /* Timeout | BadFrame | Exception(code) */ }
}
```

Behavior guarantees (all contract-derived, all DST-verified):

* waits ≥ t3.5 of bus silence before transmitting (including after boot);
* transmits request characters back-to-back (never > t1.5 apart);
* detects the response frame by t3.5 of RX silence, validates CRC/unit/fc;
* on corruption or timeout retries up to `retries` times, never earlier than
  the response timeout allows;
* returns wrong data **never** — failures are explicit (`MasterError`).

`MasterConfig::skip_pre_tx_silence` is a **planted-bug knob for negative
validation only** (it makes the driver violate RTU-T35-FRAME so tests can
prove the oracle catches it). Never enable it in real configurations.

## 3. Reference slave (`RtuSlave`) and the bug catalog

```rust
use stablehal_modbus::{RegisterBank, RtuSlave, SlaveBugs, SlaveConfig};

let bank = RegisterBank::with_pattern(|addr| 0xA000 | addr); // 128 holding registers
let slave = RtuSlave::new(
    SlaveConfig {
        unit: 0x11,
        timing,
        response_delay: Nanos::from_millis(1), // extra think time beyond t3.5
        bugs: SlaveBugs::default(),            // reference behavior
    },
    bank,
);
// drive with slave.poll(now, &mut bus) → SlaveProgress { wake_at }
```

Semantics: exception-check order per AP §7 (function → value → address);
requests to other units, CRC-garbage and runt fragments are discarded
silently; writes mutate the bank; the response starts at
`request_end + t3.5 + response_delay`.

`SlaveBugs` is the negative-validation catalog — each flag breaks **exactly
one clause** (CRC is recomputed after deliberate field mutations so failures
don't cascade):

| Bug flag | Breached clause |
|---|---|
| `respond_inside_t35` | RTU-T35-FRAME |
| `corrupt_crc` | RTU-CRC |
| `intra_frame_gap: Some(gap > t1.5)` | RTU-T15-INTRA |
| `late_by: Some(..)` | RTU-RESP-TIME |
| `wrong_fc` | RTU-RESP-FC |
| `wrong_unit` | RTU-RESP-UNIT |

## 4. PDU toolbox (`pdu` module)

* `Adu` — 256-byte no-alloc frame buffer (`bytes()`, `push*`, `append_crc`,
  `byte_mut` for deliberate corruption in tests).
* Builders: `build_request`, `build_read_response`,
  `build_write_single_response`, `build_write_multiple_response`,
  `build_exception`.
* Validating parsers: `parse_request` (slave side; yields
  `ParsedRequest`, distinguishes discard-worthy `FrameFault`s from
  answer-with-exception cases like `UnknownFunction`) and `parse_response`
  (master side; checks CRC, unit echo, fc echo / `fc|0x80` exceptions).
* `crc::crc16` / `crc::check` — CRC-16/MODBUS, low byte first on the wire.

## 5. The RTU wire contract (`RtuContract`)

The contract consumes character events with direction —
`CharStart{dir, byte}`, `CharEnd{dir, byte}`, plus a final
`EndOfObservation` — and enforces the 12 clauses listed (with spec citations)
in [architecture.md §5.4/§6.3](architecture.md) and the whitepaper's Table 1.

Judging a wire in the sim is one line:

```rust
use stablehal_conformance::runner::rtu_observer;
sim.add_observer(rtu_observer(timing, /*judge_master*/ true, /*judge_slave*/ true));
```

The `judge_*` flags implement waiver semantics: a conformance tester that
*deliberately* sends a CRC-corrupt request sets `judge_master = false` — its
garbage is treated as protocol-meaningless noise (poisoned frames), while the
DUT direction remains fully judged. Never waive the DUT.

Attribution details worth knowing:

* The oracle judges **what each party transmitted**; environment corruption
  (injected by the sim between TxEnd and delivery) is never blamed on an
  endpoint.
* An early response (< t3.5 after the request) surfaces as RTU-T35-FRAME —
  the frame-separation clause is checked before the response-envelope clause,
  so the *late* edge is what RTU-RESP-TIME distinctively reports.
* Master retries are legal only after `response_timeout`; earlier
  retransmission is RTU-MASTER-WAIT.

## 6. Demo binaries

```sh
cargo run --release -p buggy-slave                 # closed loop: reference passes 13/13,
                                                   # 6 bug kinds caught with exact clause
cargo run --release -p modbus-master -- --seeds 10000 --corrupt 2000
cargo run --release -p modbus-master -- --replay-check 4242
```
