# StableHAL — Architecture Specification (arc42)

Version 0.1.0 — updated 2026-07-11 — status: MODBUS RTU/I2C/LIN/SPI/CAN closed-loop pilots implemented (M0–M4, WP2, T8/T10), hardware capture/certify path validated in simulation, envelope containment certification piloted on MODBUS RTU/I2C/LIN (ADR-11, T11), on-target firmware drafted but unverified (§7.2).

This document follows the [arc42](https://arc42.org) template. Code references
point into this repository; the scientific argument lives in
[paper/whitepaper.md](../paper/whitepaper.md).

---

## 1. Introduction and Goals

### 1.1 Requirements overview

Embedded software validated against HAL-level simulation still fails on real
hardware, forcing a costly integration test per release. Root cause: a HAL is
an interface *signature*, not an *agreement* — it binds neither side to
behavior, ordering, timing, or fault envelopes.

StableHAL's requirement set:

| Id | Requirement |
|----|-------------|
| R1 | The HAL boundary is governed by an **executable contract**: state machine + timing envelopes + closed fault vocabulary, each clause citing the governing spec. |
| R2 | A **simulation environment** explores the *entire* contract-allowed envelope deterministically (seeded), not a single happy-path trace. |
| R3 | The **same oracle** judges simulated traces and hardware captures (`no_std`), closing the SW↔HW loop. |
| R4 | A **conformance suite** is derived from the contract and can run against simulated and real devices alike. |
| R5 | Every failure **replays byte-for-byte from its seed**. |
| R6 | Pilot protocols: MODBUS RTU over UART (FC 03/06/10 subset, unicast) — full closed loop, incl. conformance suite and (simulation-validated) hardware capture path; I2C, LIN, SPI, and CAN — full closed loop, contract + reference driver/device + simulator world + DST-verified conformance suite; IPC (request/response datagrams over a bounded queue) — full closed loop in simulation **and** a real local `AF_UNIX` capture path (§5.7), the first non-wire domain. |

### 1.2 Quality goals (top 3)

| Priority | Quality | Scenario |
|---|---|---|
| 1 | **Determinism / reproducibility** | Re-running any seed produces an identical wire trace (asserted by FNV-1a trace hash). No wall clock, no OS entropy, no hash-order dependence anywhere. |
| 2 | **Verdict precision** | Every violation names one clause, with measured-vs-required values and a spec citation; a conforming implementation is never falsely accused. |
| 3 | **Portability of judgement** | Contract kernel, protocol contracts, drivers and conformance case definitions are `no_std`, so on-target execution requires no rework of the judging code. |

### 1.3 Stakeholders

| Stakeholder | Concern |
|---|---|
| Device-driver developers | Write drivers against `UartBus`-style traits; test across the full envelope on the host. |
| Hardware/board vendors | Certify a board revision once against the conformance suite. |
| Application developers | Trust that "passes DST + certified HW" composes without per-release integration testing. |
| Researchers (whitepaper) | Reproducible measurements; explicit limits of the transitivity claim. |

---

## 2. Constraints

| Id | Constraint | Rationale |
|---|---|---|
| C1 | Rust, edition 2021. | Type-state, `no_std` targets, host+MCU from one codebase. |
| C2 | **Zero external dependencies** in all `crates/*`. | Determinism auditability; nothing outside our control injects entropy or platform variance. |
| C3 | Judging code (`stablehal-contract`, `-uart`, `-modbus`, conformance case definitions) is `no_std`. | R3. |
| C4 | Virtual time only (`Nanos`, u64 nanoseconds); wall clock is forbidden in every crate. | R5. |
| C5 | One PRNG stream (`SplitMix64`) per run, seeded from the run's seed; it is the *only* entropy source. | R5. |
| C6 | Integer arithmetic only in timing derivations (`Nanos::mul_ratio`); every component derives bounds through the same functions. | Simulator, driver, and oracle must agree to the nanosecond. |

---

## 3. Context and Scope

### 3.1 Business context

```mermaid
flowchart LR
    subgraph StableHAL
        K[Contract kernel]
        SIM[Deterministic simulator]
        CONF[Conformance suite]
    end
    DRV[Driver / application code<br>written against HAL traits] -->|exercised by| SIM
    SPEC[Protocol specification<br>MODBUS SL V1.02, AP V1.1b3] -->|cited by clauses| K
    SIM -->|verdicts, seeds, trace hashes| CI[CI pipeline / developer]
    CONF -->|pass/fail per clause| CERT[Per-HW-revision certificate]
    HW[Real device - deferred phase] -->|captured traces| K
    CONF -.->|same cases, on-target| HW
```

### 3.2 Technical scope

**In scope, closed-loop pilots (contract + reference driver/device +
simulator world + DST closed-loop test):** UART line contract; MODBUS RTU
(§5.4); I2C transaction contract (§5.1, `docs/user-guide-i2c.md`); LIN
transaction contract (§5.5, `docs/user-guide-lin.md`); SPI wire contract
(`stablehal-spi`, `docs/user-guide-spi.md`) — `SpiWorld`, `ScriptedSpiMaster`,
`SpiSlaveBugs`, a 3-case conformance suite, `tests/spi_dst.rs` and
`tests/spi_closed_loop.rs` (T8); CAN data-link contract (Classic 2.0B,
`stablehal-can`, `docs/user-guide-can.md`) — `CanWorld`, `CanNodeActor`
topology (broadcast bus, no master/slave), `CanNodeBugs`, a 3-case
conformance suite, `tests/can_dst.rs` and `tests/can_closed_loop.rs` (T8).
MODBUS additionally has a full conformance-suite runner, a validated
(simulation-only) hardware capture path (`stablehal-shc1` +
`tools/stablehal-certify`, §7.2), and an envelope containment pilot (ADR-11,
`docs/user-guide-containment.md`) — also piloted on I2C, LIN, and SPI
(T11/T12, §5.6) — proving the DST loop actually exercised the timing region a
captured trace occupies, not just that both sides pass their own contract
separately.

**ADR-10 (contract-only is a valid intermediate state)** remains accepted
policy, but no protocol crate in scope is currently in that state: SPI and
CAN closed the DST loop in T8 (2026-07-11), joining MODBUS/I2C/LIN. The
`embedded-hal` ecosystem bridge (`stablehal-embedded-hal`, spi/can
feature-gated) continues to bridge both.

**IPC (§5.7)** is the first pilot outside the wire-bus domain: a
request/response datagram channel over a bounded queue (`stablehal-ipc`,
`docs/user-guide-ipc.md`) — `IpcContract` (9 clauses), `ScriptedRequester`,
`IpcResponderDevice` + 6-bug catalog, `stablehal_sim::ipc::IpcWorld`, a
5-case conformance suite with real negative cases, `tests/ipc_dst.rs`,
`tests/ipc_closed_loop.rs`, `tests/ipc_envelope.rs`. Closed loop **in
simulation only**: there is no SHC1 capture path or `stablehal-certify`
contract code for IPC, so a real socket cannot yet be certified (§5.7,
user-guide-ipc.md §9).

**Out of scope (documented, deliberate):** physical layer (EMI, analog
margins, silicon errata outside the fault vocabulary); MODBUS broadcast
(unit 0) and function codes beyond 03/06/10; I2C multi-master arbitration
(`I2C-ARBITRATION` reserved); LIN classic checksum (enhanced only); CAN FD
(v2); on-target execution beyond the drafted, unverified `targets/rp2040/`
firmware (§7.2).

---

## 4. Solution Strategy

| Problem | Strategy |
|---|---|
| Sim ≠ HW | Don't imitate hardware; **contractualize** it. Hardware nondeterminism becomes explicit envelopes (`TimeBound`) and a closed fault vocabulary; the simulator samples the envelope, seeded. |
| Trust in the simulator | The wire is *conforming by construction* **and** still judged online by the same oracle — the claim is checked every run, not assumed. |
| Trust in hardware | The conformance suite derives from the clause list and is judged by the identical `no_std` oracle over captured traces. |
| Integration guarantee | Transitivity: DST proves SW ⊨ C over the envelope; certification proves HW ⊨ C; hence SW+HW interoperate to the extent C is complete (stated limit). |
| Tester must be able to cheat | Judgement is per-direction (`judge_master` / `judge_slave` flags): a conformance tester deliberately violates master-side clauses while the DUT stays fully judged. |

---

## 5. Building Block View

### 5.1 Level 1 — workspace

```mermaid
flowchart TD
    subgraph no_std["no_std (host + target)"]
        CONTRACT[stablehal-contract<br>kernel: Contract, Oracle,<br>Clause, Nanos, PRNG, trace hash]
        UART[stablehal-uart<br>UartBus trait + LineContract]
        MODBUS[stablehal-modbus<br>RtuContract, RtuMaster,<br>RtuSlave, pdu, crc, timing]
        I2C[stablehal-i2c<br>I2cContract, ScriptedMaster,<br>RegisterPointerDevice]
        LIN[stablehal-lin<br>LinContract, ScheduleMaster,<br>TableSlave]
        SPI[stablehal-spi<br>SpiContract, ScriptedSpiMaster,<br>SpiSlaveBugs]
        CAN[stablehal-can<br>CanContract, CanNodeActor,<br>CanScriptedNode, CanRegisterNode]
        IPC[stablehal-ipc<br>IpcContract, ScriptedRequester,<br>IpcResponderDevice]
        EHAL[stablehal-embedded-hal<br>embedded-hal bridge, spi/can features]
        SHC1[stablehal-shc1<br>capture stream codec, no_std+alloc]
        CONFCORE[stablehal-conformance core<br>Case / Stimulus / Expected,<br>STANDARD_SUITE, i2c suite]
    end
    subgraph host["host only"]
        SIM[stablehal-sim<br>Sim engine, i2c::I2cWorld,<br>lin::LinWorld, spi::SpiWorld,<br>can::CanWorld, ipc::IpcWorld]
        RUNNER[stablehal-conformance::runner<br>TesterActor, SlaveActor,<br>ScriptedMaster, run_suite]
        EX1[examples/modbus-master<br>DST sweep CLI]
        EX2[examples/buggy-slave<br>closed-loop demo CLI]
        CERTIFY[tools/stablehal-certify<br>SHC1 -> certificate CLI]
    end
    UART --> CONTRACT
    MODBUS --> CONTRACT
    MODBUS --> UART
    I2C --> CONTRACT
    LIN --> CONTRACT
    SPI --> CONTRACT
    CAN --> CONTRACT
    IPC --> CONTRACT
    EHAL --> SPI
    EHAL --> CAN
    SHC1 --> CONTRACT
    SHC1 --> UART
    SHC1 --> MODBUS
    SIM --> CONTRACT
    SIM --> UART
    SIM --> I2C
    SIM --> LIN
    SIM --> SPI
    SIM --> CAN
    SIM --> IPC
    CONFCORE --> MODBUS
    RUNNER --> CONFCORE
    RUNNER --> SIM
    RUNNER --> MODBUS
    EX1 --> RUNNER
    EX2 --> RUNNER
    CERTIFY --> SHC1
    CERTIFY --> MODBUS
    CERTIFY --> RUNNER
```

Dependency rule: arrows only point "down" toward the kernel. `stablehal-sim`
never depends on `stablehal-modbus`; protocol knowledge reaches the simulator
only through the `BusObserver` interface.

**I2C (WP2 §2.2)** is a second, independent protocol stack added alongside
MODBUS, deliberately not sharing code beyond the kernel: `stablehal-i2c`
(`no_std`: `I2cContract`, `I2cMaster`/`I2cSlaveDevice` traits,
`RegisterPointerDevice`, `ScriptedMaster`) depends only on
`stablehal-contract`, mirroring `stablehal-modbus`'s shape but with its own
event enum (`I2cEvent`, never merged with `LineEvent`). `stablehal-sim`
gained a second, self-contained world — `stablehal_sim::i2c::I2cWorld` — that
builds its own `Scheduler<Job>` the same way `Sim` does, rather than reusing
`Sim`'s 2-actor UART-shaped engine. `stablehal-conformance::i2c` follows the
same `no_std`-core / `runner`-feature split as the MODBUS suite. See
`docs/user-guide-i2c.md` for the full API walkthrough and clause table.

**LIN (`stablehal-lin`)** is a third, independent protocol stack, same
shape again: depends only on `stablehal-contract`, its own event enum
(`LinEvent`), and `stablehal_sim::lin::LinWorld` as a third self-contained
`Scheduler<Job>` world. Structurally distinct from both priors — no
arbitration (unlike I2C), no request/response coupling (unlike MODBUS): the
master's only recurring obligation is a conforming header on schedule (TDMA
via a fixed schedule table), while whichever slave owns a frame ID (if any)
carries a separate, independently-judged obligation to answer correctly.
See `docs/user-guide-lin.md` for the full walkthrough. Has a closed-loop DST
test (`crates/stablehal-sim/tests/lin_dst.rs`) and, since T10, a
`stablehal_conformance::lin` case suite (`tests/lin_closed_loop.rs`).

**SPI and CAN (`stablehal-spi`, `stablehal-can`)** closed the DST loop in
T8 (2026-07-11), joining MODBUS/I2C/LIN as full pilots. `stablehal-spi` is
split into `contract.rs`/`device.rs`/`master.rs`: `SpiContract` (CS
timing/mode discipline/mode-fault, byte granularity), `ScriptedSpiMaster`,
`SpiSlaveBugs`, and `stablehal_sim::spi::SpiWorld` (1 master, N
chip-selected slaves) back a 3-case conformance suite
(`tests/spi_dst.rs`, `tests/spi_closed_loop.rs`). `stablehal-can` is split
into `contract.rs`/`node.rs`: `CanContract` (ISO 11898-1 Classic 2.0B frame
fields, bit-stuffing, CRC, ACK, error frames — CAN FD deferred to v2) plus a
`node::CanNodeActor` trait modeling CAN's genuinely different broadcast
topology (no master/slave — any node offers a frame, every other node
ACKs), `CanNodeBugs`, `CanScriptedNode` (tester), `CanRegisterNode`
(responder, an application-level convention layered on top of the raw
contract). `stablehal_sim::can::CanWorld` (arbitration by lowest `CanId`,
seed-jittered frame duration) backs a 3-case conformance suite
(`tests/can_dst.rs`, `tests/can_closed_loop.rs`). Some clauses on both
protocols remain untested through the closed loop because the reference
world gives no observation hook to breach them (SPI-MODE-FAULT,
SPI-DATA-VALID — the latter vestigial, declared but never checked in
`step`; CAN-FRAME-START, CAN-EOF, CAN-CRC, CAN-ERROR-FRAME, CAN-OVERLOAD,
CAN-BIT-STUFFING, which need bus-monitor-level events this reference world
never emits) — see the Clause registry below and `docs/user-guide-spi.md`
/ `docs/user-guide-can.md` for the exact per-clause coverage.
`stablehal-embedded-hal` bridges StableHAL's contract-aware traits to the
Rust `embedded-hal` ecosystem (`spi`/`can` feature-gated adapters) so
existing `embedded-hal` drivers can eventually be exercised under DST.

#### Clause registry

The contract/version/clause-count/certificate-code/DST-loop index is
maintained once in **§13.3** (the single source of truth); the per-clause id
lists and citations for each protocol follow here.

Envelope containment (§5.6, ADR-11) is implemented for the timing clauses of
`modbus-rtu`, `i2c`, `lin`, and (since T12) `spi` — those durations can be
measured, not just judged pass/fail, and compared against what the DST loop
actually exercised. `can` has no timing-envelope clause (frame duration is a
world-level jitter, not a contract bound), so containment does not apply to
it.

I2C v1 clauses (full text and citations in `docs/user-guide-i2c.md` §5):
`I2C-SEQ`, `I2C-BUS-FREE`, `I2C-BYTE-TIME`, `I2C-READ-NACK-LAST`,
`I2C-ADDR-ACK`, `I2C-STRETCH-MAX`, `I2C-WRITE-ACK`. `I2C-ARBITRATION` is
reserved for v2 multi-master support and is not implemented.

LIN v1 clauses (full text and citations in `docs/user-guide-lin.md` §6):
`LIN-BREAK`, `LIN-BREAK-DELIM`, `LIN-SYNC`, `LIN-PID-PARITY`,
`LIN-FRAME-MAXLEN`, `LIN-INTERBYTE-MAX`, `LIN-CHECKSUM`,
`LIN-SLAVE-SILENCE`, `LIN-ONE-RESPONSE`.

SPI clauses (`crates/stablehal-spi/src/lib.rs` doc comment):
`SPI-CS-TIMING`, `SPI-MODE-DISC`, `SPI-XFER-TIME`, `SPI-DATA-VALID`,
`SPI-MODE-FAULT`, `SPI-TRANSACTION`, `SPI-CS-MIN-IDLE`.

CAN clauses (`crates/stablehal-can/src/lib.rs` doc comment):
`CAN-FRAME-START`, `CAN-ID-FIELD`, `CAN-RTR-DISC`, `CAN-CRC`, `CAN-ACK`,
`CAN-EOF`, `CAN-ERROR-FRAME`, `CAN-BIT-STUFFING`, `CAN-OVERLOAD` (overload
frame handling deferred).

### 5.2 Level 2 — domain model (kernel)

```mermaid
classDiagram
    class Nanos {
        +u64 0
        +from_micros(u64) Nanos
        +from_millis(u64) Nanos
        +saturating_sub(Nanos) Nanos
        +mul_ratio(num, den) Nanos
    }
    class TimeBound {
        +Nanos min
        +Nanos max
        +contains(Nanos) bool
        +exact(Nanos) TimeBound
        +at_least(Nanos) TimeBound
    }
    class Clause {
        +str id
        +str citation
        +str text
    }
    class Breach {
        +Clause clause
        +Option~Nanos~ measured
        +Option~TimeBound~ required
    }
    class Contract {
        <<trait>>
        +type State
        +type Event
        +initial() State
        +step(state, at, event) Result_Breach
        +step_measured(state, at, event, sink) Result_Breach
        +clauses() ClauseList
    }
    class Measurement {
        +str clause_id
        +Nanos value
    }
    class ClauseHull {
        +str clause_id
        +Nanos min
        +Nanos max
        +u64 samples
        +contained_in(ClauseHull) bool
    }
    class EnvelopeAccumulator~N~ {
        +observe(Measurement)
        +merge(EnvelopeAccumulator~N~)
        +hulls() Iterator_ClauseHull
    }
    class Oracle~C~ {
        -C contract
        -State state
        -usize steps
        +observe(at, event) Result_Violation
    }
    class Violation~E~ {
        +Breach breach
        +Nanos at
        +usize step
        +E event
    }
    class TraceEvent~E~ {
        +Nanos at
        +E event
    }
    class SplitMix64 {
        +next_u64() u64
        +range_inclusive(lo, hi) u64
        +chance(num, den) bool
        +pick(TimeBound) Nanos
    }
    Contract <|.. LineContract : uart
    Contract <|.. RtuContract : modbus
    Oracle~C~ o-- Contract
    Oracle~C~ ..> Violation~E~ : yields
    Breach --> Clause
    Violation~E~ --> Breach
    Contract ..> Breach
    TimeBound --> Nanos
    SplitMix64 ..> TimeBound : samples envelope
    Contract ..> Measurement : reports (step_measured)
    EnvelopeAccumulator~N~ o-- ClauseHull
    EnvelopeAccumulator~N~ ..> Measurement : folds
```

### 5.3 Level 2 — simulator

```mermaid
classDiagram
    class Sim {
        -SimConfig config
        -Nanos now
        -BinaryHeap queue
        -SplitMix64 prng
        -PortCore ports2
        -Actor actors2
        -Oracle line_oracles2
        -Vec~BusObserver~ observers
        -Vec~TraceEvent~ trace
        +run_until(deadline) Result_RunSummary
        +add_observer(obs)
        +trace_hash() u64
    }
    class SimConfig {
        +u64 seed
        +UartConfig uart
        +TimeBound latency
        +u32 corrupt_ppm
    }
    class Actor {
        <<trait>>
        +poll(now, port) NextPoll
    }
    class NextPoll {
        <<enum>>
        At(Nanos)
        OnRx
        Done
    }
    class UartPort {
        +try_write(byte) Result_TxBusy
        +try_read() Option_Result
    }
    class UartBus {
        <<trait>>
        +try_write(byte) Result_TxBusy
        +try_read() Option_Result
    }
    class BusObserver {
        <<trait>>
        +on_event(at, line, event) Result_String
        +on_end(at) Result_String
    }
    class ContractObserver~C_F~ {
        -Oracle~C~ oracle
        -F map
        -Option end_event
    }
    Sim o-- SimConfig
    Sim o-- Actor
    Sim o-- BusObserver
    Sim ..> UartPort : creates per poll
    UartBus <|.. UartPort
    BusObserver <|.. ContractObserver~C_F~
    ContractObserver~C_F~ o-- Oracle
    Actor <|.. ScriptedMaster : conformance runner
    Actor <|.. SlaveActor : conformance runner
    Actor <|.. TesterActor : conformance runner
```

Key mechanism: drivers (`RtuMaster`, `RtuSlave`) are `no_std` poll-driven
state machines written **only** against `UartBus` + a `now: Nanos` parameter.
The sim adapts them via thin `Actor` wrappers. On target the same drivers are
polled from an ISR/loop with a hardware timebase.

### 5.4 Level 2 — MODBUS building blocks

| Block | File | Responsibility |
|---|---|---|
| `crc` | [crc.rs](../crates/stablehal-modbus/src/crc.rs) | CRC-16/MODBUS + `check()` (wire order low-first). |
| `timing` | [timing.rs](../crates/stablehal-modbus/src/timing.rs) | `RtuTiming`: t1.5 / t3.5 (fixed values > 19200 baud), response envelope. |
| `pdu` | [pdu.rs](../crates/stablehal-modbus/src/pdu.rs) | `Adu` (256-byte, no-alloc frame buffer), request/response builders and validating parsers. |
| `contract` | [contract.rs](../crates/stablehal-modbus/src/contract.rs) | `RtuContract`: 12 clauses over `CharStart/CharEnd/EndOfObservation` events; per-direction judgement; frame assembly with poisoning semantics for waived violations. |
| `master` | [master.rs](../crates/stablehal-modbus/src/master.rs) | `RtuMaster`: WaitSilence → Sending → AwaitResponse FSM, t3.5 frame detection, retries, planted-bug knob. |
| `slave` | [slave.rs](../crates/stablehal-modbus/src/slave.rs) | `RtuSlave` + `RegisterBank` + `SlaveBugs` catalog (each bug breaks exactly one clause). |

### 5.5 Level 2 — LIN building blocks

| Block | File | Responsibility |
|---|---|---|
| `timing` | [timing.rs](../crates/stablehal-lin/src/timing.rs) | `LinTiming`: break/delimiter minimums, byte time, header/response/frame budgets — all derived from one nominal `bit_time`. |
| `contract` | [contract.rs](../crates/stablehal-lin/src/contract.rs) | `LinContract`: 9 clauses over frame-field events (`Break`/`Sync`/`Pid`/`DataByte`/`Checksum`); `enhanced_checksum`, `encode_pid`/`pid_parity_ok` helpers. |
| `master` | [master.rs](../crates/stablehal-lin/src/master.rs) | `ScheduleMaster`: TDMA schedule-table driver — send header, wait out response window, advance to the next slot. No request/response coupling (unlike MODBUS). |
| `device` | [device.rs](../crates/stablehal-lin/src/device.rs) | `TableSlave` (fixed signal-table publisher) + `LinSlaveBugs` catalog (`corrupt_checksum`, `respond_to_unowned`, `silent_on_owned`). |

`stablehal_sim::lin::LinWorld` executes one scheduled slot per engine step
(not byte-at-a-time like I2C's addressing phase): emit the jittered header,
then either the jittered response burst + checksum or `NoResponse`. See
`docs/user-guide-lin.md` §7 for a documented pitfall found during
development — break/delimiter timing is floor-only (no contractual
maximum), so the engine must jitter with `jitter_at_least(min)` rather than
the symmetric jitter used for byte/inter-byte gaps, or the reference master
can occasionally breach its own contract.

### 5.6 Level 2 — envelope containment (MODBUS RTU pilot, ADR-11)

| Block | File | Responsibility |
|---|---|---|
| `measure` | [measure.rs](../crates/stablehal-contract/src/measure.rs) | `Measurement`, `ClauseHull`, `EnvelopeAccumulator<N>` — `no_std`, fixed-capacity per-clause min/max/sample fold. |
| `oracle::check_trace_measured` | [oracle.rs](../crates/stablehal-contract/src/oracle.rs) | Mirrors `check_trace`; additionally folds every `step_measured` report into an `EnvelopeAccumulator`. |
| `RtuContract::step_measured` | [contract.rs](../crates/stablehal-modbus/src/contract.rs) | Reports T3.5/T1.5/master-wait/response-time durations to the sink *before* the bound check, so passing and waived measurements still contribute to the hull. |
| `sim_envelope` | [runner.rs](../crates/stablehal-conformance/src/runner.rs) | Drives the reference master/slave over N seeds, maps the wire trace to `RtuEvent`s, judges it with `check_trace_measured` — the sim-explored hull. |
| `envelope` | [envelope.rs](../tools/stablehal-certify/src/envelope.rs) | `stablehal-envelope-v1` hand-rolled JSON (G1): the hull format shared by sim and hardware sources. |
| `contain` | [contain.rs](../tools/stablehal-certify/src/contain.rs) | Per-clause `COVERED`/`UNCOVERED { gap_low_ns, gap_high_ns }`/`UNPROBED` verdict; `stablehal-certificate-v2` JSON. |

CLI: `stablehal-certify sim-envelope --seeds N`, `measure <captures>...`,
`contain --hw --sim` (plus the pre-existing `judge`, still the no-subcommand
default for compatibility). Full walkthrough and the composition argument
this closes: `docs/user-guide-containment.md`.

Since T11 (2026-07-11), the same `step_measured` / `EnvelopeAccumulator`
mechanism is also implemented by `I2cContract` and `LinContract`
(`step` delegates to `step_measured` with a no-op sink, mirroring
`RtuContract` exactly), with `sim_envelope` equivalents in
`stablehal_conformance::i2c::runner` and `::lin::runner`. I2C measures
`I2C-BUS-FREE`/`I2C-STRETCH-MAX`; LIN measures `LIN-BREAK`,
`LIN-BREAK-DELIM`, `LIN-INTERBYTE-MAX`, `LIN-FRAME-MAXLEN` — the clauses
actually judged inside each contract's `step`. Since T12, `SpiContract`
extends the same mechanism (measuring `SPI-CS-MIN-IDLE`/`SPI-XFER-TIME`),
with a `sim_envelope` in `stablehal_conformance::spi::runner` and
`tests/spi_envelope.rs`. CAN is deliberately not extended: it has no
timing-envelope clause (frame duration is world-level jitter, not a contract
bound), so there is no hull to contain (see §11).

### 5.7 Level 2 — IPC channel (the first non-wire pilot)

| Block | File | Responsibility |
|---|---|---|
| config/traits | [lib.rs](../crates/stablehal-ipc/src/lib.rs) | `IpcConfig` (three deployment profiles), `ChannelKind`, the closed `IpcFault` set, `Datagram`, `IpcRequester`/`IpcResponder` traits. |
| `contract` | [contract.rs](../crates/stablehal-ipc/src/contract.rs) | `IpcContract`: 9 clauses over datagram-level events; per-direction judgement; per-correlation-id transaction table (fixed capacity, `no_std`). |
| `requester` | [requester.rs](../crates/stablehal-ipc/src/requester.rs) | `ScriptedRequester`: the only reference driver in the workspace that must handle **backpressure** (retry-then-give-up, bounded). |
| `device` | [device.rs](../crates/stablehal-ipc/src/device.rs) | `IpcResponderDevice` + a 6-entry `IpcResponderBugs` catalog, each mapping to exactly one clause. |
| world | [ipc.rs](../crates/stablehal-sim/src/ipc.rs) | `IpcWorld`: bounded queue, seeded scheduler-latency draws, seeded peer-death injection. |
| capture format | [stablehal-shc1](../crates/stablehal-shc1/src/lib.rs) | `IpcHeader`/`IpcRecord` (datagram granularity, contract code 0x0004), sharing SHC1's SYNC-byte resync-on-corruption scheme with the RTU record shape via the header's `contract` selector. |
| capture port | [ipc-capture-shim](../tools/ipc-capture-shim/src/lib.rs) | `RealSocketRequester`/`RealSocketResponder`: real `AF_UNIX` transport implementing `IpcRequester`/`IpcResponder`, so the unmodified reference driver/device can be pointed at a real socket. Closes the pilot's capture gap (§11 T12). |

**Why this belongs in a HAL-contract project.** The five prior pilots are
wire buses: physical media, crystal-derived timing, electrical
nondeterminism. IPC has none of that *when both processes are real-time
scheduled on one host* — and that is precisely the case where it needs no
help. The two deployments that do are the ones this contract targets:

* **Non-real-time scheduling** makes responder turnaround a genuine
  `min..=max` envelope (10 µs..=20 ms in `local_non_realtime()`) rather than
  a fixed number — a 2000×-wide envelope where a wire bus has ±5%.
* **Cross-host transport** widens delivery to milliseconds and adds fault
  terms a local pipe cannot produce (duplicate, reorder).

Both are bounded nondeterminism, so both are contractualizable by §4's
strategy without modification. What genuinely *is* new, and what justifies a
separate contract rather than a re-parameterized existing one:

| Concern | Wire buses | IPC |
|---|---|---|
| Fault vocabulary | framing, parity, noise, overrun | peer death, backpressure, truncation, duplicate, reorder — **zero overlap** |
| Endpoint liveness | a device is always present | a peer can die mid-transaction, permanently |
| Flow control | none / clock stretching | bounded queue ⇒ backpressure is a first-class *legal* outcome |

`IPC-BACKPRESSURE` and `IPC-PEER-DEATH` have no analogue in MODBUS/I2C/LIN/
SPI/CAN. A MODBUS slave cannot cease to exist mid-frame; a SPI master is
never told "try again later". Modeling those two is the substance of this
pilot.

**Granularity:** datagram-level — one whole message is one event. This is
where IPC software contracts live (a caller sends a request and awaits a
response; it never observes partial frames). Truncation is therefore an
explicit fault term rather than a byte-level framing state.

**`ChannelKind` gates clause applicability.** A local kernel queue cannot
reorder or duplicate, so `IPC-ORDERING`/`IPC-NO-DUPLICATE` are not judged on
`LocalQueue` — claiming to have tested them locally would be a false coverage
claim. Symmetrically, a `Duplicate`/`Reordered` fault *reported* on a
`LocalQueue` is flagged as a model error, since the transport cannot do that.

**Containment (ADR-11)** covers `IPC-DELIVERY` and `IPC-RESP-TIME`. It earns
more here than on any wire pilot: a wire hull is narrow and predictable, so
containment nearly always holds; a scheduler-latency hull spans orders of
magnitude and depends on machine load, so "did the sim explore where
production runs?" is a genuinely open question. `sim_envelope_sweep(n)` folds
n seeds into the comparison region, and `tests/ipc_envelope.rs` proves a
still-conforming production hull outside the swept region is correctly
reported *uncontained*.

**Real-socket capture (T12, closed 2026-09-14).** `IpcContract` was always
`no_std` and judges any trace regardless of origin, but until now nothing
produced one from a real running process pair — the software analogue of
`targets/rp2040`'s hardware-capture gap, minus the hardware. A capture
format (SHC1's second record shape, contract code 0x0004) plus a real-socket
`IpcRequester`/`IpcResponder` port (`tools/ipc-capture-shim`) close that:
`tests/ipc_containment_from_capture.rs` runs the unmodified reference
driver/device over a real `AF_UNIX` socket, certifies the capture
CONFORMING, and compares its measured `IPC-DELIVERY` hull against
`sim_envelope_sweep` — correctly reported *uncovered*, since a real kernel
round trip between two OS processes costs more than an in-memory queue ever
could. That is the concrete payoff containment exists for: CI's simulated
envelope and a real socket's actual one are no longer two separate,
unverified claims.

Full walkthrough, clause table, and limits: `docs/user-guide-ipc.md`.

---

## 6. Runtime View

### 6.1 One MODBUS transaction under DST

```mermaid
sequenceDiagram
    participant M as ScriptedMaster (actor 0)
    participant E as Sim engine
    participant O as Oracles (2x Line + RTU observer)
    participant S as SlaveActor (actor 1)

    M->>E: poll @ t0 → try_write(unit)
    E->>O: TxStart line0 (judged)
    Note over E: TxEnd scheduled at t0 + char_time
    E->>O: TxEnd line0 (judged)
    E->>E: latency := prng.pick(envelope)<br>corrupt? := prng.chance(ppm)
    E->>S: Deliver at t0+char+latency → poll
    Note over M,S: ... 8 request characters back-to-back ...
    S->>S: t3.5 silence ⇒ frame complete,<br>parse, service RegisterBank
    S->>E: try_write(response bytes) after t3.5 + delay
    E->>O: response chars (judged: RESP-TIME, unit, fc, CRC)
    E->>M: Deliver response → poll
    M->>M: t3.5 silence ⇒ response complete,<br>parse_response, verify property
    Note over O: any breach ⇒ SimError with clause,<br>run aborts, seed reproduces it
```

### 6.2 DST sweep (activity)

```mermaid
flowchart TD
    A[for seed in 0..N] --> B[build script from seed<br>reads, writes, readbacks]
    B --> C[Sim::new seed, uart, envelope, ppm]
    C --> D[attach rtu_observer judge both dirs]
    D --> E[run_until virtual deadline]
    E -->|Err: Violation| F[report seed + clause<br>replay with same seed reproduces]
    E -->|Ok| G{property failures?<br>wrong data returned}
    G -->|yes| F
    G -->|no| H[record trace hash, counters]
    H --> A
    F --> X[VERDICT: nonconforming]
    A -->|done| Y[VERDICT: conforming across explored envelope]
```

### 6.3 RTU wire contract — line phase FSM

```mermaid
stateDiagram-v2
    [*] --> Silent
    Silent --> InFrame : CharStart<br>[gap >= t3.5 else RTU-T35-FRAME]<br>[proto checks: RESP-TIME / MASTER-WAIT / UNSOLICITED]
    InFrame --> InFrame : CharStart same dir<br>[gap <= t1.5 else RTU-T15-INTRA]
    InFrame --> InFrame : CharEnd (accumulate byte)
    InFrame --> Silent : gap >= t3.5 ⇒ finalize<br>[FRAME-MIN, CRC, PDU-FORM,<br>RESP-UNIT, RESP-FC, EXC-CODE]
    InFrame --> InFrame : CharStart other dir, gap < t3.5<br>⇒ RTU-T35-FRAME (barge-in)
    InFrame --> Silent : EndOfObservation ⇒ finalize
```

Protocol overlay (`Proto`): `ExpectRequest ⇄ AwaitResponse{unit, fc,
request_end}` — set by a finalized valid master frame, cleared by a finalized
slave response or a legal (post-timeout) retry.

### 6.4 Master driver FSM

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> WaitSilence : start(unit, request)
    WaitSilence --> Sending : now >= last_activity + t3.5
    Sending --> Sending : one char per char_time, back-to-back
    Sending --> AwaitResponse : last char done<br>deadline := end + response_timeout
    AwaitResponse --> Idle : t3.5 after last RX byte,<br>response parses ⇒ Done(Ok / Exception)
    AwaitResponse --> WaitSilence : poisoned / bad frame / timeout<br>and attempts left (retry)
    AwaitResponse --> Idle : attempts exhausted ⇒ Done(Err)
```

---

## 7. Deployment View

### 7.1 Current (v0.1): single host process

```mermaid
flowchart LR
    subgraph Host["developer machine / CI runner (any OS, x86-64/arm64)"]
        subgraph P["one process, one thread, virtual time"]
            ENG[Sim engine]
            A0[Actor 0: master driver]
            A1[Actor 1: slave]
            OR[online oracles]
        end
        T[cargo test] --> P
        CLI[modbus-master / buggy-slave CLIs] --> P
    end
```

No threads, no I/O, no clocks: determinism holds on any host. A 10,000-seed
sweep (~9.9M judged wire events) completes in ~10 s single-core; sweeps are
embarrassingly parallel across seeds if ever needed.

### 7.2 Target (hardware phase — capture/certify built and tested in simulation; firmware drafted, unverified on hardware)

```mermaid
flowchart LR
    subgraph HostPC["host PC"]
        CAP[capture.shc1<br>SHC1 stream]
        CERTIFY[stablehal-certify<br>parse -> RtuContract oracle]
        REP[certificate JSON: verdict,<br>trace hash, contract version]
        CAP --> CERTIFY --> REP
    end
    subgraph TesterBoard["tester board (RP2040 / RP Zero) — targets/rp2040, UNVERIFIED"]
        TESTER["tester binary<br>unmodified RtuMaster,<br>emits SHC1 over UART1"]
        UARTIF[UART0 real PL011 peripheral]
    end
    subgraph DUTBoard["DUT board (RP2040 / RP Zero) — targets/rp2040, UNVERIFIED"]
        DUTBIN["dut-slave binary<br>unmodified RtuSlave,<br>conforming config"]
    end
    TESTER -->|USB/UART: SHC1 bytes| CAP
    UARTIF ---|RS-485 / UART, crossed| DUTBoard
```

**Built and verified in simulation (E7, whitepaper §7):** the SHC1 binary
capture format (`stablehal-shc1`: header/record codec, resync-on-corruption
rule) and `tools/stablehal-certify` (parses a capture, judges it with the
*same* `RtuContract` oracle used in simulation). Round-trip identity — sim
verdict == certifier-from-capture verdict, for the conforming reference and
all 6 `SlaveBugs` — is proven by
`crates/stablehal-shc1/tests/round_trip.rs` before any firmware exists.
`stablehal-certify` additionally has `measure`/`sim-envelope`/`contain`
subcommands (§5.6, ADR-11) that build and compare timing hulls from either
source through the identical `stablehal-envelope-v1` format — proven by
`tools/stablehal-certify/tests/containment.rs`, which shows a hardware hull
matching the sim's own reference traffic is `COVERED`, and one operating
outside it (still contract-conforming) is correctly `UNCOVERED`.

**Drafted, not yet verified:** `targets/rp2040/` (standalone crate, targets
`thumbv6m-none-eabi`, not a workspace member) has `Rp2040UartBus` (`UartBus`
over the real PL011 peripheral), a `dut-slave` binary (unmodified
`RtuSlave` driven over real UART0), and a `tester` binary (unmodified
`RtuMaster` drives one transaction, streams an SHC1 header +
`EndOfObservation` over UART1). None of this has been compiled, flashed, or
run — no `thumbv6m-none-eabi` target or RP2040/RP Zero hardware was
available when it was written; `rp2040-hal` 0.12 method names are from
memory and likely need fixing on first real build. Known incomplete piece:
the tester does not yet timestamp individual `TxStart`/`TxEnd` wire events
into SHC1 records (needs either a PIO-based independent bus observer, or
per-byte instrumentation of `Rp2040UartBus`) — captures are framing-valid
but presently carry no protocol content. See `targets/rp2040/README.md`
"Status" for the itemized gap list and bring-up plan.

---

## 8. Crosscutting Concepts

### 8.1 Determinism (the load-bearing concept)

1. **Virtual time only.** `Nanos` is the sole time type; `std::time` is not
   imported anywhere.
2. **Single entropy source.** `SplitMix64::new(seed)`; draws happen in event
   order (defined by the queue), so consumption order is itself deterministic.
3. **Total event order.** The scheduler orders by `(at, seq)`; `seq` is the
   insertion counter — no `HashMap` iteration, no OS scheduling.
4. **Stable fingerprints.** FNV-1a over the canonical `Debug` rendering of the
   trace (integers only) — platform-independent hash, asserted equal on replay.

### 8.2 Contract semantics

* **Envelopes, not points**: every timing term is a `TimeBound`; a spec that
  says "at least t3.5" becomes `at_least(t35)`; tolerance terms (UART char
  time ±2%) are explicit contract parameters.
* **Closed fault vocabulary**: `UartError {Framing, Parity, Noise, Overrun}`
  is exhaustive; the environment may only inject from this set, and drivers
  must tolerate exactly this set.
* **Per-direction judgement with waiver semantics**: a waived (unjudged)
  breach *poisons* the affected frame — it carries no protocol meaning, which
  models how real receivers discard garbage — while the judged party remains
  fully accountable.
* **Judgement of transmitter truth**: oracles consume `TxStart/TxEnd`
  (what a party emitted), never `Delivered*` (what the channel did to it), so
  environment corruption is never blamed on an endpoint.

### 8.3 Error handling & reporting

`Breach` (clause + measured + required) → `Violation` (adds time, step,
offending event) → rendered with spec citation. Anything that fails renders a
seed and is reproducible; honest driver failures (retries exhausted) are
*results*, not violations.

### 8.4 Testing strategy

| Layer | Where | What |
|---|---|---|
| Unit | each crate `#[cfg(test)]` | CRC spec vector, timing derivations, contract clause triggers, PRNG stability. |
| DST harness self-test | [dst.rs](../crates/stablehal-sim/tests/dst.rs) | Planted envelope-dependent bug found by sweep; exact replay; seed divergence. |
| Closed loop (MODBUS) | [closed_loop.rs](../crates/stablehal-conformance/tests/closed_loop.rs) | Reference passes suite; 6/6 bug catalog caught with exact clause; master sweep clean; planted master bug caught. |
| Closed loop (I2C) | [i2c_dst.rs](../crates/stablehal-sim/tests/i2c_dst.rs), [i2c_closed_loop.rs](../crates/stablehal-conformance/tests/i2c_closed_loop.rs) | Reference passes 50-seed envelope; each `I2cSlaveBugs` entry caught with exact clause (or correctly *not* flagged, for the data-only `wrong_read_data` bug); replay-exact. |
| Closed loop (LIN) | [lin_dst.rs](../crates/stablehal-sim/tests/lin_dst.rs), [lin_closed_loop.rs](../crates/stablehal-conformance/tests/lin_closed_loop.rs) | Reference passes 50-seed envelope; `corrupt_checksum`/`silent_on_owned` caught with exact clause; replay-exact; since T10, 4-case `STANDARD_SUITE` passes with declared-clause coverage asserted. |
| Closed loop (SPI) | [spi_dst.rs](../crates/stablehal-sim/tests/spi_dst.rs), [spi_closed_loop.rs](../crates/stablehal-conformance/tests/spi_closed_loop.rs) | Reference passes 50-seed envelope; `wrong_read_data` correctly *not* flagged (data-only bug); replay-exact; 3-case suite covers SPI-TRANSACTION/CS-TIMING/MODE-DISC/XFER-TIME/CS-MIN-IDLE. |
| Closed loop (CAN) | [can_dst.rs](../crates/stablehal-sim/tests/can_dst.rs), [can_closed_loop.rs](../crates/stablehal-conformance/tests/can_closed_loop.rs) | Reference tester+responder pass a 50-seed envelope; `silent_no_ack` caught as CAN-ACK, `wrong_response_data`/`silent_no_response` correctly *not* flagged; replay-exact; 3-case suite covers CAN-ID-FIELD/CAN-RTR-DISC/CAN-ACK. |
| Envelope containment | [envelope.rs](../crates/stablehal-conformance/tests/envelope.rs), [i2c_envelope.rs](../crates/stablehal-conformance/tests/i2c_envelope.rs), [lin_envelope.rs](../crates/stablehal-conformance/tests/lin_envelope.rs), [spi_envelope.rs](../crates/stablehal-conformance/tests/spi_envelope.rs), [containment.rs](../tools/stablehal-certify/tests/containment.rs) | `sim_envelope` measures every timing clause `RtuContract`/`I2cContract`/`LinContract`/`SpiContract` judge inside `step` on a conforming reference run, and asserts a nonempty hull per clause; a "hardware" capture matching the sim's own reference traffic is `COVERED`, one operating outside it (still contract-conforming) is `UNCOVERED` — proves the check has teeth, not just that it type-checks. |
| Closed loop (IPC) | [ipc_dst.rs](../crates/stablehal-sim/tests/ipc_dst.rs), [ipc_closed_loop.rs](../crates/stablehal-conformance/tests/ipc_closed_loop.rs) | Reference pair passes a 50-seed envelope on all three deployment profiles; `wrong_correlation`/`never_answer` caught with the exact clause across 10 seeds each; `wrong_payload` correctly *not* flagged by the contract but caught by the property check on `requester_outcomes()`; backpressure exercised and survived; seeded peer death surfaced (not hung) and replay-exact; the scheduling envelope is asserted to be actually wide (>4× spread in run duration across seeds), so a collapse of envelope exploration fails loudly. |
| Envelope containment (IPC) | [ipc_envelope.rs](../crates/stablehal-conformance/tests/ipc_envelope.rs) | Both timing clauses produce nonempty hulls; measured hulls sit inside their declared envelopes (proving the ADR-11 side channel has not drifted from the judging path); a 50-seed sweep hull spans >10×; a still-conforming "production" hull beyond the swept region is correctly reported **uncontained**. |
| Capture round-trip | [round_trip.rs](../crates/stablehal-shc1/tests/round_trip.rs) | Sim-side oracle verdict == certifier-from-SHC1-capture verdict, for the reference and all 6 `SlaveBugs` — validates the hardware-capture path before any firmware exists. |
| Measurement | example CLIs | 10k-seed sweep, replay check — numbers feed the whitepaper. |

---

## 9. Architecture Decisions (ADRs)

| # | Decision | Status | Rationale / consequences |
|---|---|---|---|
| ADR-1 | Contracts judge **wire truth per direction**, not deliveries. | accepted | Correct fault attribution under injected corruption; conformance testers can deliberately violate their own direction. |
| ADR-2 | Drivers are **poll-driven `no_std` state machines** (`poll(now, bus)`), not async. | accepted | Runs identically under sim scheduling and on bare metal; no executor dependence; wake-hint (`wake_at`) keeps sims efficient. |
| ADR-3 | **Zero external crates** in `crates/*`. | accepted, with one deliberate exception | Auditable determinism; cost: hand-rolled PRNG/hash (~60 LoC), no rmodbus in core (adapter lives outside `crates/`). Exception: `stablehal-embedded-hal` depends on `embedded-hal`/`embedded-io`/`embedded-can` — its entire purpose is bridging to that ecosystem, so the exception is the point, not a lapse; every other `crates/*` member (`stablehal-shc1` included) still depends only on `stablehal-contract` and, where relevant, `stablehal-uart`/`stablehal-modbus`. |
| ADR-4 | Simulator is **conforming by construction *and* self-judged**. | accepted | The "environment contract" claim is verified every run; a sim bug surfaces as a line-oracle violation (`SimError::Line`). |
| ADR-5 | Sim topology fixed at **2 devices / 2 simplex lines** for v0.1. | accepted | Covers master/slave pilot; multi-drop & shared buses are handled *not* by extending `Sim` but by a separate `Scheduler`-based world per protocol (`I2cWorld`, `LinWorld`) — see ADR-6/ADR-10 and risk T4. |
| ADR-6 | Higher-layer contracts attach via **`BusObserver`**, keeping `sim` protocol-agnostic. | accepted | `stablehal-sim` never imports modbus; new protocols plug in without touching the engine. |
| ADR-7 | Frame-oriented contracts get an explicit **`EndOfObservation`** event. | accepted | A trailing frame cannot escape judgement; harnesses must append it (the sim does this in `run_until`). |
| ADR-8 | Waived breaches **poison** frames instead of halting. | accepted | Lets one oracle instance serve both strict DST (judge all) and conformance probing (judge DUT only). |
| ADR-9 | `Adu` is a fixed 256-byte buffer (spec max), cloned freely. | accepted | No alloc in `no_std`; clippy "large enum variant" warnings accepted knowingly. |
| ADR-10 | A protocol crate may ship **contract-only** (clause set + `Contract` state machine, no reference driver/device/sim world) as a valid intermediate state. | accepted | Remains accepted policy, but no longer describes any in-scope protocol's current status: `stablehal-spi`/`stablehal-can` closed the full DST loop in T8 (2026-07-11), joining `stablehal-modbus`/`stablehal-i2c`/`stablehal-lin`. All five pilot protocols now have a reference driver + `stablehal-sim` world + DST closed-loop test and may be cited as pilot evidence; ADR-10 stays in force as the policy for any *future* protocol crate added in a contract-only state. |
| ADR-11 | Timing measurement is a **side channel of judging**, not a second implementation of the bounds: `Contract::step_measured` defaults to delegating to `step` with a no-op sink, and a contract that overrides it has `step` call back into `step_measured` — exactly one code path computes a duration and checks it. | accepted | Prevents the sim-explored hull and the hardware-measured hull from silently drifting apart from the pass/fail verdict they're supposed to explain. Originally piloted on `RtuContract` only (§5.6); since T11 (2026-07-11) also piloted on `I2cContract`/`LinContract`, and since T12 on `SpiContract`. The CAN contract has no timing-envelope clause, so it stays on the trait default (empty hull) by design, not by omission. |

---

## 10. Quality Requirements (scenarios)

| Quality | Scenario | Measure (v0.1 actual) |
|---|---|---|
| Reproducibility | Re-run any failing seed | identical trace hash + identical violation (asserted in CI) |
| Bug-finding power | Planted ~1%-per-transaction timing bug | found within 200 seeds; single mock trace misses it |
| Verdict precision | 6 cataloged slave bugs | 6/6 attributed to the exact clause; reference 13/13 pass |
| Throughput | 10,000-seed sweep | ~10 s single core, ~9.9M events judged |
| Safety property | 0.2%/char corruption injected | 0 silent data corruptions; failures are loud (retry-then-error) |

---

## 11. Risks and Technical Debt

| Id | Risk / debt | Mitigation |
|---|---|---|
| T1 | **Contract completeness** — a wrong-but-consistent contract passes both sides. | Clause↔spec traceability table; boundary probes at both envelope edges; treat field divergence as contract bugs (fix upgrades all three artifacts). |
| T2 | Envelope coverage is sampled, not exhausted. | Explicit envelope parameters; scale seeds in CI; boundary-biased sampling (`SplitMix64::pick_biased`, opt-in via `SimConfig::boundary_bias` / `modbus-master --bias`) to concentrate draws on envelope edges. |
| T3 | Response-timeout race (spec ambiguity: retry may collide with a maximally-late response). | Contract makes the race visible; deployments configure margins; documented in whitepaper §6. |
| T4 | `Sim` (the original UART/MODBUS engine) is hardcoded to 2 devices / 2 simplex lines. | Resolved for multi-drop protocols by *not* extending `Sim` — `I2cWorld`/`LinWorld` are separate `Scheduler`-based engines supporting N devices via `add_slave` (ADR-6/ADR-10 pattern). `Sim` itself stays 2-device; do not extend it ad hoc. |
| T5 | `expected_request_len` in [slave.rs](../crates/stablehal-modbus/src/slave.rs) exists only to implement a *bug* (early response); keep it out of reference paths. | Code comment + conformance case GAP-T15-ACCEPTED guards the legal path. |
| T6 | Clippy warnings (large enum variants, complex runner types). | Accepted for v0.1 (ADR-9); revisit if `Adu` clones show up in profiles. |
| T7 | The conformance tester's listening window is length-aware but static. | Fine at 9600–115200 baud; recompute per case if very slow bauds are added. |
| T8 | ~~`stablehal-spi`/`stablehal-can` are contract-only (ADR-10) — no reference driver, no `stablehal-sim` world, no DST closed-loop test, no conformance suite.~~ **Closed 2026-07-11.** | Resolved: reference driver + `stablehal-sim` world (`SpiWorld`/`CanWorld`) + DST closed-loop test + 3-case conformance suite added for both SPI and CAN; see `ChangeLog.txt` 2026-07-11 entry ("T10/T11/T8: LIN suite, envelope containment for I2C/LIN, SPI + CAN closed-loop pilots") and §3.2/§5.1. SPI/CAN may now be cited as pilot evidence, with the per-clause coverage caveats noted in §5.1. Was tracked as backlog; now reflected in `next_steps.md` as done. |
| T9 | `targets/rp2040/` firmware (T7 hardware phase) is drafted but **unverified**: never compiled (no `thumbv6m-none-eabi` installed), never flashed, `rp2040-hal` API calls written from memory. | See `targets/rp2040/README.md` "Status" for the itemized gap list; fix on first real build once hardware is available. |
| T10 | ~~`stablehal-lin` has a closed-loop DST test but no `stablehal_conformance::lin` case suite yet (unlike MODBUS/I2C).~~ **Closed 2026-07-11.** | Resolved: `stablehal_conformance::lin` added (`mod.rs` + `runner.rs`), 4-case `STANDARD_SUITE`, `tests/lin_closed_loop.rs`; see `ChangeLog.txt` 2026-07-11 entry and §5.1. |
| T11 | ~~Envelope containment (§5.6, ADR-11) is piloted on `modbus-rtu` timing clauses only; I2C/LIN/SPI/CAN contracts still use the `step_measured` trait default (empty hull) — containment cannot be claimed for them yet.~~ **Closed for every applicable protocol** (I2C/LIN 2026-07-11 as T11; SPI as T12). CAN has no timing-envelope clause, so containment does not apply to it (documented, not deferred). Containment is min/max only, not a statistical/percentile guarantee — that scope limit remains. | Resolved: `I2cContract`/`LinContract`/`SpiContract` implement `step_measured` mirroring `RtuContract`; `sim_envelope` added to `stablehal_conformance::{i2c,lin,spi}::runner`; `tests/i2c_envelope.rs`/`tests/lin_envelope.rs`/`tests/spi_envelope.rs`. See `ChangeLog.txt` and §5.1/§9 (ADR-11). Document the min/max-only scope in any external certificate language rather than overstating it as a probabilistic bound. |
| T12 | ~~The IPC pilot (§5.7) is closed-loop **in simulation only**: `IpcContract` judges any trace, but no capture format or capture-producing tool exists for a real socket — the software analogue of `targets/rp2040` (T9)'s "specified, not demonstrated" gap.~~ **Closed 2026-09-14.** | Resolved: `stablehal-shc1` gained a second record shape for IPC (contract code 0x0004, §13.3), and `tools/ipc-capture-shim` provides a real-`AF_UNIX`-socket `IpcRequester`/`IpcResponder` implementation the unmodified reference driver/device can run against with no code of their own aware of the difference — no interposer, no `unsafe`. `stablehal-certify` gained `merge-ipc` and IPC dispatch in `judge`/`measure`/`sim-envelope`. `tests/ipc_containment_from_capture.rs` proves the acceptance bar: a real capture's `IPC-DELIVERY` hull compared against `sim_envelope_sweep` via `contain` is correctly reported uncovered — sim's in-memory-queue delivery latency is genuinely tighter than a real kernel round trip. Remaining scope: one host, one requester, one responder; cross-host capture and multi-requester fan-in are still simulation-only, same as `ChannelKind::CrossHost`'s standing limits. See `ChangeLog.txt` 2026-09-14 entry and `docs/user-guide-ipc.md` §9–§10. |

---

## 12. Glossary

| Term | Meaning |
|---|---|
| **ADU / PDU** | MODBUS Application Data Unit (unit + PDU + CRC) / Protocol Data Unit (fc + data). |
| **Backpressure** | IPC: a send rejected because the bounded receive queue is full. A *legal* outcome the sender must survive by retrying or shedding load — not a failure (`IPC-BACKPRESSURE`). |
| **Bounded nondeterminism** | Real-world variance expressed as an explicit contract envelope (`TimeBound`) or closed fault set, so it can be sampled and judged. |
| **Break field / break delimiter** | LIN: the master's dominant-then-recessive preamble (>=13 bit times low, >=1 bit time high) that opens every frame. |
| **Certificate** | JSON verdict emitted by `stablehal-certify`: contract id/version, UART framing, derived timing envelope, capture record/gap counts, trace hash, verdict (v1: `judge`); v2 additionally reports per-clause containment (`contain`). |
| **Clause** | One checkable obligation, with stable id and spec citation. |
| **Clause hull** | The observed `min..=max` and sample count for one timing clause across a trace or a folded set of traces (`ClauseHull`, §5.6). |
| **Conforming by construction** | The simulated wire physically cannot emit contract-forbidden character patterns (and is still judged online). |
| **Contract-only crate** | A protocol crate shipping a clause set + `Contract` state machine but no reference driver/device/sim world yet (ADR-10) — a real artifact, but has not closed the DST loop. |
| **DST** | Deterministic simulation testing: all nondeterminism seeded and pluggable; failures replay from the seed. |
| **DUT** | Device under test (conformance). |
| **Envelope** | The `min..=max` band of allowed values for a timing term. |
| **Envelope containment** | The check that a hardware-measured clause hull sits inside the sim-explored clause hull — proof the DST loop tested where the hardware actually operates, not just that both sides pass their own contract separately (§5.6, ADR-11, `docs/user-guide-containment.md`). |
| **Oracle** | Folds a timestamped event trace through a contract; yields conformance or the breached clause. |
| **PID (Protected Identifier)** | LIN: the 6-bit frame ID plus 2 parity bits computed per the LIN 2.2A formula, sent as one byte after sync. |
| **Poisoned frame** | Frame stripped of protocol meaning after a waived (unjudged) breach — models receivers discarding garbage. |
| **Scheduling-latency envelope** | IPC: the `min..=max` band a non-real-time responder's turnaround falls in, dominated by OS scheduler wakeup rather than by the work itself. Orders of magnitude wider than any wire-bus envelope. |
| **Schedule table** | LIN: the fixed, TDMA-ordered list of frame IDs the master polls in sequence — LIN has no arbitration, so this replaces bus contention entirely. |
| **SHC1** | The binary capture stream format (`stablehal-shc1`): header + fixed-size records, resync-tolerant, judged by the same oracle as simulation. |
| **t1.5 / t3.5** | MODBUS RTU character-time multiples bounding intra-frame gaps / delimiting frames (fixed 750 µs / 1.75 ms above 19200 baud). |
| **Trace hash** | FNV-1a fingerprint of the canonical wire trace; equality across runs proves replay exactness. |
| **Waiver** | Direction excluded from judgement so a tester may deliberately violate; never applied to the DUT. |

---

## 13. Engineering ground rules and golden values

These are binding invariants of the codebase. A change that violates one is a
review blocker; a change that legitimately alters a golden value must say so in
`ChangeLog.txt` and update the value here in the same commit.

### 13.1 Ground rules

| Id | Rule |
|----|------|
| G1 | `crates/*` carry **zero external dependencies** (ADR-3). Third-party code and MCU HALs live only under `adapters/`, `targets/`, `tools/`. The single exception is `stablehal-embedded-hal`, whose entire purpose is bridging to the `embedded-hal` ecosystem. |
| G2 | **Determinism** (§8.1): no wall clock; one seeded `SplitMix64` per run; every random draw happens in event order; total event order is `(at, seq)`. |
| G3 | Every new clause carries `id` + spec `citation` + one-sentence `text`, and is exercised by **at least one conformance case and one negative test** that proves the oracle catches its violation. |
| G4 | Timing derivations go through shared `const fn`s (`UartConfig::char_time`, `Nanos::mul_ratio`, `RtuTiming`/`I2cTiming`/`LinTiming` constructors) — never duplicated arithmetic, so simulator, driver, and oracle agree to the nanosecond. |
| G5 | Public API additions get doc comments; a contract-version-affecting change gets an explicit `ChangeLog.txt` entry. |

### 13.2 Golden values (assert; never silently change)

| Check | Command | Value |
|---|---|---|
| Replay determinism | `modbus-master --replay-check 4242` | trace hash `95f0e863d22deaa9` |
| 10k-seed sweep (seed 0..10000, 2000 ppm) | `modbus-master --seeds 10000` | 0 violations, 0 property failures, 11 give-ups, 9,876,528 events |
| CRC-16/MODBUS vector | unit test | `01 03 00 00 00 01` → wire CRC `84 0A` |
| SplitMix64 seed 0, first output | unit test | `0xe220a8397b1dcdaf` |
| SPI DST replay determinism | `cargo test -p stablehal-sim --test spi_dst` | same-seed trace hash equal across two runs, different-seed hashes differ (`SpiWorld::trace_hash`) — no literal hash pinned here, unlike MODBUS, since there is no `--replay-check`-style CLI surface for SPI yet |
| CAN DST replay determinism | `cargo test -p stablehal-sim --test can_dst` | same-seed trace hash equal across two runs, different-seed hashes differ (`CanWorld::trace_hash`) — no literal hash pinned here, unlike MODBUS, since there is no `--replay-check`-style CLI surface for CAN yet |

### 13.3 Contract/certificate registry

| Contract | Version | Clauses | Certificate `contract` code | DST closed loop |
|---|---|---|---|---|
| uart-line | 0.1 | 3 | 0x0002 | via modbus-rtu |
| modbus-rtu | 0.1 | 12 | 0x0001 | yes |
| i2c | 0.1 | 7 | 0x0003 (reserved) | yes |
| lin | 0.1 | 9 | unassigned | yes |
| spi | — | 7 | unassigned | yes |
| can | — | 9 | unassigned | yes |
| ipc | 0.1 | 9 | 0x0004 | yes |

The ultimate source of truth is the `Clause` registry inside each protocol
crate (`clauses()`); this table is the single documentation index for it (§5.1
points here rather than repeating the table). Certificate codes: `modbus-rtu`
(0x0001) and `ipc` (0x0004) are wired into `stablehal-certify` today; the
others are reserved/unassigned. `ipc`'s code deliberately skips ahead of
`lin`/`spi`/`can` — precedented by `i2c`'s own out-of-pilot-order reservation
of 0x0003. See §5.1 for the per-clause id lists and citations.
