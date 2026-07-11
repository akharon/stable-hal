# User Guide — Conformance Suite (`stablehal-conformance`)

Audience: anyone certifying a MODBUS RTU slave implementation (simulated
today, hardware later), and maintainers adding cases.

## 1. What a "case" is

A case = scripted wire stimuli + an expectation + judgement flags. The case
definitions are `no_std` (`Case`, `Step`, `Stimulus`, `Expected` in
[lib.rs](../crates/stablehal-conformance/src/lib.rs)); only the runner is
host-side. The same definitions will drive the on-target runner.

**Suite precondition** (part of the suite's specification): the DUT's holding
registers are initialized to `pattern(addr) = 0xA000 | addr`, and the DUT is
configured as unit `DUT_UNIT = 0x11`. A hardware fixture must arrange the
same before a certification run.

## 2. Running the standard suite

```rust
use stablehal_conformance::runner::{run_suite, SlaveActor, SuiteConfig};
use stablehal_conformance::{pattern, DUT_UNIT, STANDARD_SUITE};
use stablehal_modbus::{RegisterBank, SlaveBugs, SlaveConfig};

let cfg = SuiteConfig::default_9600(); // 9600-8E1, 50 ms response timeout, fixed seed
let report = run_suite(&cfg, STANDARD_SUITE, || {
    Box::new(SlaveActor::new(
        SlaveConfig {
            unit: DUT_UNIT,
            timing: cfg.timing,
            response_delay: stablehal_contract::Nanos::from_millis(1),
            bugs: SlaveBugs::default(),
        },
        RegisterBank::with_pattern(pattern), // ← the precondition
    ))
});
assert!(report.all_pass(), "{}", report.failure_text());
```

The factory closure is called **once per case** — every case starts from a
freshly initialized DUT, so cases cannot contaminate each other (writes in
WRITE-\* cases don't leak into READ-\* cases).

## 3. Reading a report

`Report.outcomes` holds one `CaseOutcome { id, pass, detail }` per case.
Failure details come in two flavors:

* **Wire contract violation** — `detail` embeds the oracle verdict:
  clause id, spec citation, measured vs required, offending event.
  Example: `wire contract violation: observer violation: clause RTU-T35-FRAME
  violated at 610.887ms (step 57): Frames must be separated by at least t3.5
  of silence. [SL §2.5.1.1] — measured 1.336ms, required 4.010ms..=∞ …`
* **Expectation mismatch** — the DUT stayed inside the wire contract but
  answered wrongly: `expected silence, DUT transmitted …`, `response […] !=
  expected […]`, `expected one response frame, saw 2`.

Both name the case id; `Report::failure_text()` concatenates them (this is
what the closed-loop tests assert clause ids against).

## 4. The standard suite at a glance

| Case | Stimulus | Expectation |
|---|---|---|
| READ-BASIC | valid FC03 ×10 | pattern values |
| READ-MAX | FC03 ×125 (max) | pattern values |
| READ-BAD-ADDR / READ-BAD-COUNT | out-of-range / zero count | exception 02 / 03 |
| WRITE-SINGLE-READBACK | FC06 then FC03 | echo, then persisted value |
| WRITE-SINGLE-BAD-ADDR | FC06 out of range | exception 02 |
| WRITE-MULTI-READBACK | FC16 then FC03 | echo, then persisted values |
| UNKNOWN-FC | CRC-valid FC 0x2B | exception 01 |
| WRONG-UNIT-SILENT | valid frame, other unit | silence |
| CRC-CORRUPT-SILENT † | flipped CRC | silence |
| RUNT-SILENT † | 3-byte fragment | silence |
| SPLIT-FRAME-SILENT † | > t3.5 gap mid-request | silence |
| GAP-T15-ACCEPTED | exactly-t1.5 gap mid-request | normal response (gap is legal!) |

† = `judge_master_dir: false`: the tester violates deliberately; only the DUT
direction is judged. The DUT is **never** waived.

Note the two timing probes are a matched pair: GAP-T15-ACCEPTED guards
against over-strict DUTs (rejecting legal frames), SPLIT-FRAME-SILENT against
over-lenient ones (accepting torn frames).

## 5. Certifying your own slave implementation

1. Adapt it behind the HAL: implement `stablehal_sim::Actor` by polling your
   stack and exchanging bytes through the provided `UartPort` (which
   implements `UartBus`). See `SlaveActor` for the shape; a worked
   third-party-crate adapter lives in
   [adapters/rmodbus-slave/](../adapters/rmodbus-slave/).
2. Ensure the precondition (unit 0x11, pattern registers).
3. `run_suite(&cfg, STANDARD_SUITE, factory)` — all-pass = certified against
   contract v0.1 at 9600-8E1/50 ms. Re-run at your deployment baud/timeout.

## 6. Adding a case (maintainers)

1. Add a `Case` to `STANDARD_SUITE`: pick a stable `id`, list the clause ids
   it `probes`, decide `judge_master_dir` (false only if the *tester* must
   break master-side rules to deliver the stimulus).
2. New stimulus shapes go into `Stimulus` (`no_std` — keep payloads
   `'static`); teach `build_stimulus` in
   [runner.rs](../crates/stablehal-conformance/src/runner.rs) to render the
   bytes/gaps, and `expected_frame` to render the expectation.
3. Sanity-check both directions of the loop: the reference slave must still
   pass 13+1 cases, and at least one `SlaveBugs` configuration (or a new one)
   should fail your case — a case no bug can fail is dead weight.
4. Timing probes: state which envelope *edge* you probe and on which side
   (legal edge → expect acceptance; illegal side → expect silence/violation).
   Mind receive-side jitter: the sim delivers with `1–50 µs` latency, so
   "just over t3.5" stimuli use a +250 µs margin (see `GapKind::WellOverT35`)
   to stay unambiguous at the receiver.

## 7. Known limits (v0.1)

* Response-window length is computed for ≤ 256-char responses at the
  configured baud; very slow custom bauds with multi-second frames would need
  a per-case window override.
* Broadcast (unit 0) and FCs outside 03/06/10 are out of subset — requests
  are still *sent* correctly by the tester (UNKNOWN-FC), but no
  broadcast-turnaround cases exist yet.
* One DUT per bus; multi-drop topologies use a separate per-protocol world
  (`I2cWorld`/`LinWorld`), not the 2-device UART `Sim` (architecture.md T4).
