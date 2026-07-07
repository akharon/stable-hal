# StableHAL — Design & Interface Specification for the Next Milestones

Version: draft 1.0 — 2026-07-07.
Status: **specification only — nothing below is implemented.**
Companion backlog with ordering and acceptance criteria: [next_steps.md](../next_steps.md).
Architecture context: [architecture.md](architecture.md) (esp. ADR-3, ADR-5, ADR-6).

Scope of this document: four work packages.

| WP | Title | New code lives in | Size (est.) |
|----|-------|-------------------|-------------|
| WP1 | Third-party MODBUS slave adapter (rmodbus) | `adapters/rmodbus-slave/` | S (~300 LoC) |
| WP2 | Scheduler extraction + I2C transaction contract | `crates/stablehal-sim`, `crates/stablehal-i2c` | L (~1500 LoC) |
| WP3 | Hardware phase: capture format, host certifier, on-target runner | `tools/`, `targets/` | L |
| WP4 | Harness hardening: clause coverage, boundary-biased sampling, verify script | kernel + conformance | S |

Ground rules that bind all WPs (violations are review blockers):

* **G1** `crates/*` keep **zero external dependencies** (ADR-3). Third-party
  code and MCU HALs are allowed only under `adapters/`, `targets/`, `tools/`.
* **G2** Determinism rules of [user-guide-simulator.md §8](user-guide-simulator.md):
  no wall clock, single seeded PRNG, total event order, all new randomness
  drawn in event order.
* **G3** Every new clause carries `id` + `citation` + one-sentence `text`,
  and appears in at least one conformance case *and* one negative test that
  proves the oracle catches its violation (both directions of the loop).
* **G4** Timing derivations go through shared `const fn`s (like
  `UartConfig::char_time`) — never duplicated arithmetic.
* **G5** Public API additions get doc comments; contract-version-affecting
  changes get a ChangeLog entry that says so explicitly.

---

## WP1 — Third-party adapter: `rmodbus` slave behind `UartBus`

### 1.1 Purpose

Headline experiment E5 for the whitepaper: take a popular ecosystem MODBUS
implementation, put it behind the StableHAL boundary *unmodified*, and run
`STANDARD_SUITE` against it. Pass or fail, the result is publishable: it
answers "does the ecosystem actually conform?".

### 1.2 Placement and dependencies

```
adapters/
  rmodbus-slave/
    Cargo.toml        # deps: stablehal-{contract,uart,modbus,sim,conformance}, rmodbus (pinned, e.g. "0.10")
    src/lib.rs        # RmodbusSlaveActor
    tests/suite.rs    # runs STANDARD_SUITE, records the outcome
```

Add to workspace `members`. Pin the `rmodbus` version exactly (`=x.y.z`) —
the whitepaper must name the tested version. `cargo` needs network access for
the first fetch; if unavailable, vendor it and note that in the ChangeLog.

### 1.3 Responsibility split (important for fair attribution)

`rmodbus` is a **frame-level** library (parse request bytes → produce
response bytes over a register context). It has no notion of t1.5/t3.5 or
character pacing. Therefore:

* **Wire-timing clauses** (T35, T15-INTRA, RESP-TIME) certify **the
  adapter's** RX-framing and TX-pacing code → reuse the already-certified
  logic patterns from `RtuSlave` (t3.5 frame detection; back-to-back TX).
* **Frame/semantic clauses** (CRC, RESP-UNIT, RESP-FC, EXC-CODE, PDU-FORM)
  and all `Expected` outcomes certify **rmodbus itself**.

State this split in the test output and the whitepaper table.

### 1.4 Interface

```rust
/// Adapter: rmodbus server behind the StableHAL UART boundary.
pub struct RmodbusSlaveActor {
    unit: u8,
    timing: RtuTiming,
    /// Extra think time beyond t3.5 before the response starts (mirror of
    /// SlaveConfig::response_delay; keep inside the response envelope).
    response_delay: Nanos,
    // internal: rx accumulation (Adu), last_rx: Option<Nanos>, poisoned: bool,
    // tx state (idx, next_at, resp buffer), rmodbus context (register storage)
}

impl RmodbusSlaveActor {
    /// `pattern` initializes the rmodbus holding-register context —
    /// the suite precondition (conformance user guide §1).
    pub fn new(unit: u8, timing: RtuTiming, response_delay: Nanos,
               pattern: fn(u16) -> u16) -> Self;
}

impl stablehal_sim::Actor for RmodbusSlaveActor { /* poll(now, port) */ }
```

Poll behavior (mirror `RtuSlave` exactly, except frame *processing*):

1. Drain `port.try_read()`: accumulate into an `Adu`; `Err(_)` sets
   `poisoned`; record `last_rx = now`.
2. On `now >= last_rx + t3.5` with a non-empty buffer: if poisoned → discard;
   else hand `adu.bytes()` to rmodbus (`ModbusFrame` parse/process against
   the context — check the pinned version's exact API, it changed across 0.x)
   and collect the response bytes, if any.
   **Do not pre-filter** unit address or CRC in the adapter — whether rmodbus
   discards wrong-unit/bad-CRC frames is precisely what the suite measures.
   Only if rmodbus *cannot* be handed raw ADUs (API forces pre-validated
   PDUs), do the minimum preprocessing its API demands and document each
   preprocessing step as "certified by adapter, not by rmodbus".
3. Response TX: start at `frame_end + t3.5 + response_delay`, characters
   back-to-back at `char_time` — copy of `RtuSlave::Responding`.

### 1.5 Test / measurement

`tests/suite.rs`:

```rust
#[test]
fn rmodbus_versus_standard_suite() {
    let cfg = SuiteConfig::default_9600();
    let report = run_suite(&cfg, STANDARD_SUITE, || Box::new(
        RmodbusSlaveActor::new(DUT_UNIT, cfg.timing, Nanos::from_millis(1), pattern)));
    // Do NOT assert all_pass. Assert the *report is stable* and print it:
    println!("{}", render_table(&report));
    // Golden expectation: once the actual outcome is known, pin it:
    // assert_eq!(report.passed(), <observed>);
}
```

First run establishes the observed outcome; then pin it as a golden value so
regressions in the adapter are caught. Record per-case results + rmodbus
version in `paper/whitepaper.md` §5 (E5).

---

## WP2 — Scheduler extraction + I2C transaction contract

### 2.1 Step A: extract the deterministic scheduler (refactor, no behavior change)

Problem: `Sim` hardcodes 2 devices / 2 UART lines (ADR-5). I2C needs a shared
multi-drop bus. Extract the reusable core first:

```rust
// crates/stablehal-sim/src/sched.rs  (new)
pub struct Scheduler<J> {
    now: Nanos,
    seq: u64,
    queue: BinaryHeap<Reverse<Scheduled<J>>>,   // ordered by (at, seq)
    prng: SplitMix64,
}
impl<J> Scheduler<J> {
    pub fn new(seed: u64) -> Self;
    pub fn now(&self) -> Nanos;
    pub fn schedule(&mut self, at: Nanos, job: J);       // clamps at >= now
    /// Pop the next job due at or before `deadline`; advances `now`.
    pub fn pop_due(&mut self, deadline: Nanos) -> Option<(Nanos, J)>;
    pub fn prng(&mut self) -> &mut SplitMix64;
}
```

Rework the existing `Sim::run_until` loop on top of `Scheduler<Job>`.
**Acceptance for step A alone:** all existing tests green AND
`modbus-master --replay-check 4242` still prints hash `95f0e863d22deaa9`
(trace-hash stability proves the refactor changed nothing observable).

### 2.2 Step B: crate `stablehal-i2c` (`no_std`, kernel + uart-independent)

Model at **transaction/byte granularity** (bytes + ACK bits + Start/Stop +
stretch durations), not SDA/SCL edges. Rationale: it is the granularity at
which SW contracts live (drivers see bytes and ACKs); edge-level belongs to
electrical compliance (out of scope, same as UART's physical layer).
**v1 scope: single master, N slaves. Multi-master arbitration is explicitly
deferred to v2** — reserve the clause id `I2C-ARBITRATION`, do not implement.

#### 2.2.1 Configuration and timing

```rust
pub enum I2cSpeed { Standard100k, Fast400k }   // v1: these two

pub struct I2cTiming {
    pub speed: I2cSpeed,
    /// SCL frequency envelope actually driven by the master (masters may run
    /// slower than nominal; never faster than the mode maximum). UM10204 §6.
    pub scl: TimeBound,            // period envelope, e.g. 10µs..=100µs for 100k
    /// Bus-free time between Stop and next Start (tBUF). UM10204 Table 10:
    /// ≥ 4.7 µs (Standard), ≥ 1.3 µs (Fast).
    pub bus_free_min: Nanos,
    /// Contract cap on cumulative clock stretching per byte, by the slave.
    /// The I2C spec allows unbounded stretching; deployments must bound it —
    /// default to the SMBus-inspired 25 ms unless configured tighter.
    pub stretch_max: Nanos,
}
impl I2cTiming {
    pub const fn standard() -> Self;   // canonical constants; single source (G4)
    pub const fn fast() -> Self;
    /// Nominal time for 9 SCL cycles (8 data bits + ACK) at a given period.
    pub const fn byte_time(scl_period: Nanos) -> Nanos; // scl_period * 9
}
```

#### 2.2.2 SW-facing HAL traits (poll-driven, same conventions as `UartBus`)

```rust
pub struct I2cBusy;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum I2cFault {
    /// No slave acknowledged the address byte.
    AddrNack,
    /// Slave NACKed a written data byte (index within this op).
    DataNack { at_byte: u8 },
    /// Cumulative stretching exceeded the contract cap.
    StretchTimeout,
}   // closed vocabulary, v1

pub enum I2cOp<'a> {
    /// START, addr+W, bytes...; STOP if `stop`, else leave bus claimed
    /// (next op begins with a repeated START).
    Write { addr7: u8, bytes: &'a [u8], stop: bool },
    /// START/reSTART, addr+R, read `len` bytes (master ACKs all but the
    /// last, NACKs the last); STOP if `stop`.
    Read { addr7: u8, len: u8, stop: bool },
}

pub trait I2cMaster {
    /// Begin an operation. Err while one is in flight.
    fn try_begin(&mut self, op: &I2cOp<'_>) -> Result<(), I2cBusy>;
    /// Completion of the in-flight op, if any. Read data is delivered here.
    /// `buf` receives read bytes (caller-provided, len from the op).
    fn poll_done(&mut self, buf: &mut [u8]) -> Option<Result<(), I2cFault>>;
}
```

Slave side (for the reference slave and DUT adapters):

```rust
/// Byte-level hooks a slave device model implements. The bus (sim or target
/// shim) drives these; timing/ACK mechanics stay in the bus layer.
pub trait I2cSlaveDevice {
    fn addr7(&self) -> u8;
    /// Master wrote a byte to us. Return whether we ACK it, and optionally
    /// how long we stretch the clock first (must respect the contract cap).
    fn on_write(&mut self, byte: u8) -> (bool /*ack*/, Nanos /*stretch*/);
    /// Master reads a byte from us.
    fn on_read(&mut self) -> (u8, Nanos /*stretch*/);
    /// Transaction delimiters (for register-pointer state machines).
    fn on_start(&mut self);
    fn on_stop(&mut self);
}
```

Reference device model (also the conformance DUT model): a 256×u8
**register-pointer device** (24Cxx-EEPROM-style subset): `Write [ptr,
data...]` sets the pointer then writes sequentially; `Read` returns bytes
from the pointer, auto-incrementing; pointer wraps at 256.

#### 2.2.3 Wire events and clauses

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum I2cEvent {
    Start,                                   // also repeated start
    AddrByte { addr7: u8, read: bool, acked: bool },
    DataByte { master_to_slave: bool, byte: u8, acked: bool },
    /// Slave-attributed cumulative stretch inside the preceding byte.
    Stretch { dur: Nanos },
    Stop,
    EndOfObservation,
}
```

Clause table (citations: NXP UM10204 rev 7; SMBus 3.x for the stretch cap):

| Clause id | Party | Obligation |
|---|---|---|
| `I2C-SEQ` | master | Well-formed sequencing: Start → AddrByte → DataBytes → (reStart \| Stop); no Stop without an open transaction; EndOfObservation with an open transaction is a breach. |
| `I2C-BUS-FREE` | master | ≥ `bus_free_min` between Stop and the next Start. (UM10204 Table 10, tBUF) |
| `I2C-BYTE-TIME` | master | Each byte (9 SCL cycles) fits `9 × scl envelope` + observed Stretch. |
| `I2C-READ-NACK-LAST` | master | The final byte of every Read is NACKed by the master; all earlier ones ACKed. (UM10204 §3.1.10) |
| `I2C-ADDR-ACK` | slave | A configured slave ACKs its own address (device-presence term of the deployment contract). |
| `I2C-STRETCH-MAX` | slave | Cumulative stretch per byte ≤ `stretch_max`. |
| `I2C-WRITE-ACK` | slave | The reference register-pointer device ACKs all in-range writes (device-model term; parameterize per DUT model). |
| `I2C-ARBITRATION` | — | **Reserved, v2** (multi-master). Do not implement; keep in `clauses()` output? No — list only implemented clauses; mention in docs. |

`I2cContract` implements `Contract` with `State` tracking: bus phase
(Free/Claimed{since}/InByte...), open-transaction descriptor (addr, dir,
bytes so far), per-byte stretch accumulator. Judgement is per-party via
flags `judge_master` / `judge_slaves`, with the same waiver→poison semantics
as `RtuContract` (reuse the pattern, not the code).

#### 2.2.4 Simulator world: `I2cWorld` (in `stablehal-sim`, new module)

```rust
pub struct I2cWorldConfig {
    pub seed: u64,
    pub timing: I2cTiming,
    /// Master's actual SCL period is drawn once per run from timing.scl.
    /// Per-byte jitter: ±1% of the period, drawn per byte.
    pub _reserved: (),
}

pub struct I2cWorld {
    // Scheduler<I2cJob>, one I2cMasterActor slot, Vec<Box<dyn I2cSlaveDevice>>,
    // Oracle<I2cContract>, trace: Vec<TraceEvent<I2cEvent>>
}
impl I2cWorld {
    pub fn new(cfg: I2cWorldConfig, master: Box<dyn I2cMasterActor>) -> Self;
    pub fn add_slave(&mut self, dev: Box<dyn I2cSlaveDevice>);
    pub fn run_until(&mut self, deadline: Nanos) -> Result<RunSummary, SimError>;
    pub fn trace(&self) -> &[TraceEvent<I2cEvent>];
    pub fn trace_hash(&self) -> u64;
}

/// Master-side actor: polled like uart actors; sees an I2cMaster port.
pub trait I2cMasterActor {
    fn poll(&mut self, now: Nanos, port: &mut dyn I2cMaster) -> NextPoll;
}
```

Engine mechanics: each in-flight op is executed byte by byte on the
scheduler; per byte the engine (a) draws jitter, (b) asks the addressed
`I2cSlaveDevice` for ack/stretch (slave chooses stretch **deterministically
from its own state**; sweep variation comes from device configs derived from
the seed), (c) emits `AddrByte`/`DataByte`/`Stretch` events through the
oracle, (d) delivers completion to the master port. Address with no matching
slave → `AddrNack` fault to the master, `acked: false` on the wire (which is
*not* an `I2C-ADDR-ACK` breach unless a slave is configured at that address —
the contract state must know the configured address set:
`I2cContract { configured_addrs: /* sorted list */ }`).

#### 2.2.5 Conformance sketch (v1 suite, DUT = slave device)

Mirror the MODBUS structure: `stablehal-conformance` gains an `i2c` module
(cases `no_std`) + runner support. Standard cases: WRITE-READBACK,
POINTER-WRAP, ADDR-ACK (probe configured address), OTHER-ADDR-SILENT (probe
neighboring address → must NACK), STRETCH-WITHIN-CAP (DUT with configured
stretch under the cap must pass), plus negative catalog `SlaveBugs`-style:
`stretch_over_cap`, `nack_own_addr`, `wrong_read_data`.

### 2.3 What NOT to do in WP2

* No SDA/SCL edge modeling, no 10-bit addressing, no multi-master, no SMBus
  PEC — all v2+.
* Do not force I2C into the UART `Sim`; do not merge `LineEvent` and
  `I2cEvent` into one enum. Shared code is the `Scheduler`, `Contract`
  kernel, and trace machinery only.

---

## WP3 — Hardware phase: capture format, certifier, on-target runner

Order within WP3: **3.1 → 3.2 can be built and tested entirely in simulation
before any hardware exists** (generate capture files from sim traces); 3.3 is
the first hardware code.

### 3.1 Capture stream format ("SHC1")

Binary, little-endian, designed so a PIO/ISR can emit it with no buffering
cleverness and the host can resynchronize after corruption.

```
Header (16 bytes, once per stream):
  magic       u32  = 0x31434853        ("SHC1")
  contract    u16  = 0x0001            (rtu v0.1; registry in this doc)
  flags       u16  = bit0: self_report (tester timestamps its own TX)
                     bit1: observer    (independent bus observer)
  baud        u32
  uart_cfg    u8   = bits[0..4] data_bits, bit4 parity_en, bit5 parity_odd,
                     bits[6..8] stop_bits
  reserved    u8[3] = 0

Record (12 bytes, repeating):
  sync        u8   = 0xA5
  kind        u8   = 0: TxStart, 1: TxEnd, 2: DeliveredOk, 3: DeliveredErr,
                     0xFF: EndOfObservation
  line        u8   = 0: master→slave, 1: slave→master
  payload     u8   = byte value (kind 0..2), UartError discriminant (kind 3)
  t_ns        u64  = virtual/monotonic nanoseconds since stream start
```

Resync rule: on parse failure scan forward to the next `0xA5` whose record
passes `kind <= 3 || kind == 0xFF` and monotonic `t_ns`; count skipped bytes
in the certificate as `capture_gaps`.

### 3.2 Host tool `tools/stablehal-certify`

CLI (std allowed; still zero third-party deps — hand-roll the trivial JSON):

```
stablehal-certify capture.shc1 --timeout-ms 50 [--json report.json]
```

Pipeline: parse SHC1 → map records to `RtuEvent`s exactly as
`rtu_observer` does (kinds 0/1 → CharStart/CharEnd with `dir` from `line`;
kinds 2/3 ignored for judgement; 0xFF → EndOfObservation) → feed
`check_trace(RtuContract::new(timing), &events)` → emit certificate:

```json
{
  "format": "stablehal-certificate-v1",
  "contract": { "id": "modbus-rtu", "version": "0.1", "clauses": 12 },
  "uart": { "baud": 9600, "framing": "8E1" },
  "timing": { "t15_ns": 1718749, "t35_ns": 4010415, "response_timeout_ns": 50000000 },
  "capture": { "records": 12345, "gaps": 0, "trace_hash": "95f0e863d22deaa9" },
  "verdict": "CONFORMING",
  "violation": null
}
```

**Round-trip test (no hardware needed):** add a sim-side SHC1 *writer*
(serialize `sim.trace()` into the format) and assert: sim verdict ==
certifier verdict, for a conforming run and for every `SlaveBugs` entry.
This pins the format and the mapping before any firmware exists.

### 3.3 On-target conformance runner (`targets/rp2040-tester`)

* Leaf crate, external deps allowed here (`rp-hal` or `embassy-rp` — choose
  one and pin; `rp-hal` suggested for less async machinery, since our drivers
  are poll-driven anyway).
* Timebase: 1 MHz hardware timer → `Nanos::from_micros(timer.get_counter())`.
  1 µs resolution is fine: the tightest envelope edge (UART char tolerance
  2% at 115200 ≈ 1.9 µs) still resolves; document that certification at
  >115200 baud needs the PIO observer's higher resolution.
* Structure: implement `UartBus` over the RP2040 UART (single-depth TX by
  polling the FIFO-empty flag; map RX error flags onto `UartError`); reuse
  the `no_std` `TesterActor` logic — **first refactor** `TesterActor` +
  `StepPlan` out of `runner.rs` into a `no_std` `tester` module in
  `stablehal-conformance` (they contain no sim types besides the Actor trait;
  introduce a tiny `no_std` port trait bound instead).
* Output: stream SHC1 over USB-CDC (self_report flag set); host runs
  `stablehal-certify`.
* v2: separate PIO observer board (both RS-485 directions passively tapped,
  edge-timestamped at 125 MHz / PIO clock, chars reconstructed on-host);
  flips the `observer` flag and removes self-report trust.

---

## WP4 — Harness hardening (small, high leverage)

### 4.1 Clause coverage accounting

Add to `stablehal-conformance`:

```rust
impl Report {
    /// Clause ids probed by the executed cases (union of case.probes).
    pub fn probed_clauses(&self, suite: &[Case]) -> Vec<&'static str>;
}
```

New test: every id in `RtuContract::clauses()` (except documented
structural-only ones: RTU-CHAR-OVERLAP) appears in some case's `probes` —
the suite cannot silently drift away from the contract.

### 4.2 Boundary-biased envelope sampling

```rust
impl SplitMix64 {
    /// 25% exact min, 25% exact max, 50% uniform interior. One extra draw
    /// for the branch selector, then at most one for the interior value —
    /// draw order fixed, so replay determinism holds.
    pub fn pick_biased(&mut self, bound: TimeBound) -> Nanos;
}
```

Use it for the sim latency draw behind a `SimConfig` flag
(`boundary_bias: bool`, default **false** — flipping the default would change
all existing trace hashes; see 2.1 acceptance).

### 4.3 `scripts/verify.sh`

```
cargo fmt --check
cargo clippy --all-targets   # -D warnings once the ADR-9 lints are allowed inline
cargo test
cargo run --release -p modbus-master -- --seeds 10000
cargo run --release -p modbus-master -- --replay-check 4242
cargo run --release -p buggy-slave
```

Exit nonzero on any failure. This is the CI gate and the whitepaper's
reproduction command.

---

## Registry (single source of truth, update on change)

| Contract | Version | Clauses | Certificate `contract` code |
|---|---|---|---|
| uart-line | 0.1 | 3 | 0x0002 |
| modbus-rtu | 0.1 | 12 | 0x0001 |
| i2c (WP2) | 0.1-draft | 7 | 0x0003 (reserve) |

Known golden values (must survive refactors, see WP2 §2.1):

| Check | Value |
|---|---|
| `modbus-master --replay-check 4242` hash | `95f0e863d22deaa9` |
| 10k sweep (seed 0..10000, 2000 ppm) | 0 violations, 0 property failures, 11 give-ups, 9,876,528 events |
| CRC vector `01 03 00 00 00 01` | wire CRC `84 0A` |
