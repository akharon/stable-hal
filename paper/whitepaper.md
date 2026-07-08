# StableHAL: Replacing Recurring Integration Tests with a Deterministic, Contract-Enforcing HAL

*Draft v0.2 — July 2026. All measurements reproduce from this repository at the stated commands.*

## Abstract

Hardware abstraction layers let embedded software run against simulation, yet
every release still ends in a costly integration test on real hardware. We
argue the root cause is not that simulators fail to mimic hardware, but that
the HAL itself is only an interface *signature*: it binds neither software nor
hardware to any behavior, ordering, timing, or fault envelope. StableHAL makes
the HAL an **executable contract** in which real hardware nondeterminism is
expressed as *bounded nondeterminism* — explicit envelopes rather than
implicit chance. One contract then yields three artifacts: (1) a reference
simulation that is conforming by construction, (2) a runtime trace oracle that
judges any execution — simulated or captured from hardware — against the same
clauses, and (3) a conformance suite generated from the clause list. Software
is exercised by deterministic simulation testing (DST) across the *entire*
allowed envelope; hardware is certified against the *same* oracle. When both
sides hold, interoperation follows by transitivity, and the recurring
per-release integration test collapses into a **one-time per-hardware-revision
conformance certification**. We demonstrate the closed loop on a MODBUS RTU
pilot: a 12-clause wire contract with spec traceability, a conforming master
driver and reference slave, a 13-case generated conformance suite, and a DST
harness that judged 9.88 million wire events across 10,000 seeded environment
schedules with zero violations, zero silent data corruptions, and byte-exact
seed replay — in under ten seconds on one laptop core. We further show the
approach generalizes: the same kernel judges an unrelated third-party MODBUS
stack (rmodbus) with zero contract changes, and a second, independently
modeled protocol (I2C — a 7-clause transaction contract covering clock
stretching, ACK discipline, and bus-free timing) closes an identical loop
end to end.

## 1. The problem: HALs are signatures, not agreements

A HAL promises `uart_write(byte)` and `uart_read() -> byte`. It does not
promise *when* a response may arrive, *how long* silence delimits a frame,
*which* faults may surface, or *what* the peer must do next. Both sides fill
the vacuum with assumptions:

* the **simulator** implements the happy path (often zero-latency, fault-free),
* the **hardware** implements physics (jitter, corruption, timing windows).

Software validated against the first routinely fails against the second, so
teams re-test the integrated stack on real hardware for every release. The
usual conclusion — "simulation can never act like hardware, because hardware
is not deterministic" — misidentifies the problem. Determinism is not a
property of hardware to replicate; it is a property of the **agreement**. If
the agreement says "the response starts between t3.5 and 50 ms after the
request, and any character may be corrupted," then:

* a simulator can *explore that whole envelope deterministically* (seeded),
* hardware can be *checked against that envelope* on a captured trace,
* and neither side can hide behind an undocumented assumption.

## 2. Related work

Each ingredient exists; to our knowledge their combination at the HAL
boundary, with the same oracle judging both simulation and hardware, does not.

* **Deterministic simulation testing.** FoundationDB and Antithesis made all
  nondeterminism pluggable and seed-driven, pairing it with fault injection;
  WarpStream applied it to a whole SaaS. Proven for distributed systems; not
  applied as a *bilateral HW/SW contract*.
* **Deterministic embedded emulation.** Renode runs full embedded systems
  deterministically, but models concrete chips rather than contracts, and has
  no generated hardware-conformance loop back.
* **Formal HW/SW interface specs.** Microsoft's P language shipped the
  Windows 8.1 USB 3.0 stack from model-checked state machines; UniTESK
  derived hardware test oracles from contract specifications; recent work
  (arXiv 2512.16897) runtime-checks STM32/PSoC HAL usage continuously —
  software side only.
* **Conformance certification.** OPC UA's Compliance Test Tool and USB-IF
  qualification show the economics StableHAL generalizes: certified
  implementations interoperate *without pairwise integration testing*.
  Notably, MODBUS has no comparably strong tooling — a gap this pilot fills.
* **Reference instruments.** PHiLIP (RIOT OS) demonstrates the
  external-reference-device pattern our deferred hardware phase adopts.

## 3. The StableHAL model

### 3.1 Contracts

A contract is a deterministic state machine over timestamped events with
three kinds of terms:

* **ordering obligations** (who may transmit what, when, in reply to what),
* **timing obligations as envelopes** `min..=max` — never point values,
* **a closed fault vocabulary** — the only faults either side may exhibit.

Every clause carries a stable identifier and a citation into the governing
specification (traceability, Table 1). Determinism requirement: `step(state,
time, event)` is a pure function. All hardware "randomness" the contract
tolerates is written down as envelope width — *bounded nondeterminism*.

### 3.2 Three artifacts from one contract

1. **Reference simulation, conforming by construction.** The simulated wire
   physically cannot emit what the contract forbids — and is still judged by
   the oracle at runtime, so the claim is checked, not assumed. One virtual
   clock; one seeded PRNG as the *only* entropy source; ties in the event
   queue broken by sequence number. A run is a pure function of (config,
   seed, actors); any failure replays byte-for-byte from its seed.
2. **Trace oracle.** Folds timestamped events through the contract and
   returns either conformance or the breached clause with measured-vs-required
   values. `no_std`: the identical code judges host simulations and on-target
   captures.
3. **Conformance suite.** Generated from the clause list: state coverage,
   boundary timing probes (e.g. an exactly-t1.5 gap that *must* be accepted,
   a beyond-t3.5 gap that *must* split the frame), each fault-vocabulary
   item, plus deliberate-violation stimuli. The tester may break master-side
   clauses on purpose; judgement of the DUT direction is never waived.

### 3.3 The transitivity argument, stated precisely

Let *C* be the contract, *S* the software stack, *E* the set of environment
behaviors *C* allows, and *H* the hardware.

* DST establishes: for all explored e ∈ E, S(e) conforms to C. (Coverage of
  E is sampled, seed-driven, and reproducible; the envelope is finite and
  explicitly parameterized.)
* Conformance certification establishes: H's observable behavior ⊆ E, and
  H's obligations under C hold on every probe.
* Therefore S over H conforms — *to the extent C is complete*.

The honest claim is **not** "no integration test ever." It is:

> the recurring, per-release integration test is replaced by (a) recurring
> DST in CI and (b) a one-time per-hardware-revision certification run,
> both judged by the same oracle.

What remains outside C — analog behavior, EMI, silicon errata not expressed
in the fault vocabulary — remains outside the guarantee, exactly as it
remains outside today's integration tests.

## 4. Pilot: MODBUS RTU

MODBUS RTU was chosen because its timing is *already contractual* (the spec
defines frames by t3.5 silence and bounds intra-frame gaps by t1.5), its
master/slave discipline avoids concurrency ambiguity, and its ecosystem lacks
strong conformance tooling. Subset: FC 0x03/0x06/0x10 + exceptions, unicast.

**Table 1 — wire contract clauses and traceability**
(SL = MODBUS over Serial Line V1.02; AP = MODBUS Application Protocol V1.1b3)

| Clause | Citation | Obligation (abbrev.) | Negative validation |
|---|---|---|---|
| RTU-T35-FRAME | SL §2.5.1.1 | ≥ t3.5 silence between frames | slave responding inside t3.5 → caught |
| RTU-T15-INTRA | SL §2.5.1.1 | ≤ t1.5 silence within a frame | slave gapping mid-response → caught |
| RTU-CHAR-OVERLAP | SL §2.5.1 | no simultaneous transmitters | (collision path) |
| RTU-FRAME-MIN | SL §2.5.1 | ≥ 4 bytes per frame | runt stimulus → DUT must stay silent |
| RTU-CRC | SL §2.5.1.2 | valid CRC-16, low byte first | CRC-corrupting slave → caught |
| RTU-PDU-FORM | AP §6 | PDU consistent with FC | malformed probes |
| RTU-RESP-UNIT | SL §2.4.1 | response echoes request unit | wrong-unit slave → caught |
| RTU-RESP-FC | AP §7 | response echoes FC or FC\|0x80 | alien-FC slave → caught |
| RTU-EXC-CODE | AP §7 | exception = 5 bytes, valid code | exception-path cases |
| RTU-RESP-TIME | SL §2.4.1 | response starts in [t3.5, timeout] | late slave → caught |
| RTU-MASTER-WAIT | SL §2.4.1 | no new request before timeout | eager master → caught |
| RTU-UNSOLICITED | SL §2.4.2 | slave speaks only when spoken to | wrong-unit/CRC/runt cases |

Beneath it, a 3-clause UART line contract (character timing, no overlap,
start/end pairing) guards the physical framing layer, with a 2% clock
tolerance envelope.

### 4.1 Second pilot: I2C transaction layer

I2C's 7 v1 clauses (`I2C-SEQ`, `I2C-BUS-FREE`, `I2C-BYTE-TIME`,
`I2C-READ-NACK-LAST`, `I2C-ADDR-ACK`, `I2C-STRETCH-MAX`, `I2C-WRITE-ACK`;
full text and NXP UM10204/SMBus 3.x citations in
`docs/user-guide-i2c.md` §5) are modeled at byte/ACK/Start-Stop granularity,
the level software contracts actually live at — mirroring the UART-line vs.
MODBUS-frame split in §4. Multi-master arbitration is reserved
(`I2C-ARBITRATION`) and explicitly unimplemented in v1. See E6 for results.

## 5. Evaluation

All numbers measured on this repository (single laptop core, debug oracle in
release build). Reproduce with the commands shown.

**E1 — DST finds envelope-dependent bugs a mock cannot.** A toy driver was
planted with a timing bug that manifests only when two propagation latencies
are both drawn small (≈1% of transactions). A 200-seed sweep finds it; the
failing seed replays with an identical trace hash and identical violation.
A single-trace mock would show it only by luck.
(`cargo test -p stablehal-sim`)

**E2 — the closed loop, positive and negative.**
(`cargo run --release -p buggy-slave`)

| Device under test | Suite result | First breached clause |
|---|---|---|
| reference slave | **13/13 pass** | — |
| responds inside t3.5 | 5/13 | RTU-T35-FRAME |
| corrupt CRC | 4/13 | RTU-CRC |
| gap > t1.5 in response | 4/13 | RTU-T15-INTRA |
| response after timeout | 4/13 | RTU-RESP-TIME |
| wrong function code | 4/13 | RTU-RESP-FC |
| wrong unit address | 4/13 | RTU-RESP-UNIT |

Every cataloged violation is caught *and attributed to the exact clause*; the
conforming implementation is not falsely accused. The same oracle also caught
a planted master-side bug (transmitting without the pre-TX t3.5 wait).

**E3 — envelope sweep of the production driver.**
(`cargo run --release -p modbus-master -- --seeds 10000`)

| Metric | Value |
|---|---|
| seeded environment schedules explored | 10,000 (10,000 distinct wire traces) |
| wire events judged online | 9,876,528 |
| transactions completed | 149,989 |
| transactions honestly abandoned (0.2%/char corruption) | 11 — contract-legal |
| wire contract violations | **0** |
| silent data corruptions | **0** |
| wall-clock time | 9.7 s |

The "honestly abandoned" row is the point of the fault vocabulary: under
injected corruption the driver may fail *loudly* (after contractual retries)
but must never return wrong data. It didn't, across the whole explored
envelope.

**E4 — determinism.** Same-seed reruns produce identical trace hashes
(`--replay-check`); the test suite asserts hash equality and cross-seed
divergence. There is no wall-clock dependence anywhere in the system
(virtual time only), and the only entropy source is the run's seed.

**E5 — an independent third-party stack, same oracle, zero contract
changes.** We swapped the reference slave's protocol logic for `rmodbus`
0.12.2 (a MIT-licensed, independently authored MODBUS implementation) behind
the identical `UartBus`/simulator boundary and ran the unchanged 13-case
suite against it. Result: **10/13 pass**; the 3 failures are *measured
differences*, not oracle bugs — rmodbus's larger register space (10k vs.
our 128-register reference) means out-of-range addresses return data
instead of exception 0x02, and its `count = 0` handling on FC03 returns one
register instead of raising exception 0x03. This is exactly the intended
outcome: the same clause-labeled oracle, unmodified, characterizes a
third-party implementation's real behavioral deviations without anyone
writing bespoke test code for it.
(`cargo test -p rmodbus-slave --test suite`)

**E6 — a second, independently modeled protocol closes the identical
loop.** I2C is a materially different protocol from MODBUS RTU: multi-drop
addressing (not point-to-point), slave-initiated clock stretching (not just
master-side timeouts), and ACK/NACK at every byte (not just frame-level
CRC). We built its contract from scratch — 7 clauses (`I2C-SEQ`,
`I2C-BUS-FREE`, `I2C-BYTE-TIME`, `I2C-READ-NACK-LAST`, `I2C-ADDR-ACK`,
`I2C-STRETCH-MAX`, `I2C-WRITE-ACK`), citing UM10204 rev 7 and SMBus 3.x —
sharing with the MODBUS pilot only the kernel primitives (`Contract`,
`Oracle`, `Nanos`/`TimeBound`, the seeded PRNG, and the `(at, seq)`-ordered
scheduler), never the event types or the simulator engine itself. Reference
master + reference register-pointer device pass the whole seeded envelope;
each of 3 planted device bugs (`stretch_over_cap`, `nack_own_addr`,
`wrong_read_data`) is caught with the exact clause named, or correctly
*not* flagged when the bug is a data-correctness issue rather than a
wire-protocol breach (`wrong_read_data` — confirming the oracle's judgement
boundary is precise, not merely permissive); seed replay is byte-exact.
(`cargo test -p stablehal-sim --test i2c_dst`)

**E7 — capture-and-certify round-trip.** We built the SHC1 capture format
and a standalone certifier tool (`tools/stablehal-certify`, zero
third-party dependencies) that parses a capture, maps records to the same
event types the simulator emits, and judges them with the identical
`RtuContract` oracle. For the conforming reference and all 6 cataloged
`SlaveBugs`, the simulator's own verdict and the certifier-from-capture
verdict agree exactly — this is the load-bearing property for §7's
deferred hardware phase: a physical capture will be judged by literal
byte-identical logic to what already validated the simulation, not a
reimplementation that could silently diverge.
(`cargo test -p stablehal-shc1 --test round_trip`)

**Effort accounting.** Contract kernel ≈ 500 LoC; UART contract ≈ 260;
simulator ≈ 440 (+ ≈80 for the extracted `Scheduler<J>`); MODBUS contract +
drivers ≈ 1,600; conformance suite + runner ≈ 930; I2C contract + reference
device/master + `I2cWorld` ≈ 1,150; SHC1 codec + certifier ≈ 340. The 12
RTU clauses cover the normative timing/framing content of ~15 pages of the
two MODBUS specs relevant to the subset; the 7 I2C clauses cover the
transaction-layer content of UM10204's normative timing table plus the
SMBus stretch-cap convention.

## 6. Limits and threats to validity

* **Contract completeness is the crux.** A contract that is conforming but
  *wrong about the spec* passes both sides and still fails in the field.
  Mitigations: per-clause spec citations (Table 1); boundary probes at both
  edges of every envelope; and the closed loop itself — any divergence found
  later is by construction either an implementation bug or a *contract* bug,
  and fixing the contract upgrades every artifact at once.
* **Coverage of E is sampled.** 10,000 seeds is not exhaustion. The envelope
  parameters are explicit, so coverage can be scaled (it is embarrassingly
  parallel) or targeted (boundary-biased draws).
* **The physical layer stays out of scope.** EMI, analog margins, silicon
  errata outside the fault vocabulary are certified today by neither our loop
  nor conventional integration tests; they belong to electrical compliance.
* **Response-timeout races.** The spec allows a master to retry exactly when
  a maximally late response may still arrive; the contract makes the race
  visible (it is an envelope-edge case), which we consider a feature —
  the ambiguity is in the spec, and deployments should configure margins.
* **Generalization is evidenced, not proven.** MODBUS RTU is favorable
  terrain (that is why it was chosen). A second, independently modeled
  contract (I2C — E6) now exists and closes the identical loop, sharing only
  kernel primitives with the first; this is evidence the approach is not an
  artifact of MODBUS specifically, but two data points do not establish a
  general theory of which protocols this scales to, and multi-master I2C
  arbitration remains explicitly out of scope (v1: single master only).

## 7. The deferred hardware phase

Nothing in the oracle or suite is simulator-specific (`no_std` throughout).
Two of the three pieces below are now built and validated entirely in
simulation (E7); the third — real hardware — remains future work.

1. **Capture format and certifier — done (E7).** SHC1 is a binary,
   resync-tolerant stream format a PIO/ISR can emit with no buffering
   cleverness; `stablehal-certify` parses it and judges captures with the
   identical oracle used in simulation. The round-trip identity (simulator
   verdict == certifier-from-capture verdict, for the reference and every
   cataloged bug) is proven now, before any firmware exists — so the
   hardware phase inherits validated codec and mapping logic rather than
   writing it under time pressure against a live board.
2. **On-target conformance runner** (RP2040 or STM32 Nucleo) — not yet
   built: the tester plays the same 13 cases over a real RS-485/UART link
   against the DUT; captured timestamps feed the identical oracle via the
   SHC1 path above.
3. **Bus observer** — not yet built: a PHiLIP-style reference instrument
   (RP2040 PIO is sufficient for 8E1 timestamping at ≤115,200 baud)
   witnesses the wire so the DUT is judged on physical truth, not on its
   own self-report.
4. **Certification artifact.** The certificate schema (contract id/version,
   UART framing, derived timing envelope, capture record/gap counts, trace
   hash, verdict) is defined and emitted by `stablehal-certify` today for
   simulator-derived captures; a hardware capture is a drop-in input once
   items 2–3 exist. Any later field divergence is triaged against the same
   clauses.

## 8. Conclusion

The pilot demonstrates, end to end and reproducibly, that the HAL boundary
can be governed by one executable contract whose simulator explores the whole
agreed envelope and whose oracle judges both parties. Two extensions
strengthen the claim beyond a single favorable protocol: the same oracle,
unmodified, characterizes a third-party implementation's real deviations
(E5), and a second, independently modeled protocol closes an identical loop
sharing only kernel primitives (E6) — evidence the pattern is not MODBUS-
specific. The capture/certify path (E7) is now validated in simulation,
ahead of hardware. On this terrain the integration test is not "eliminated"
— it is *transformed*: from a recurring, manual, irreproducible activity
into (a) seeded CI sweeps and (b) a one-time per-revision certification,
both pinned to clauses a counterparty signed.

---

*Repository layout, build and reproduction commands: see README.md.*
