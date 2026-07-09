# User Guide — Envelope containment certification (`stablehal-certify`)

Audience: anyone using StableHAL's "test SW against the simulator, test HW
against the contract oracle, separately" story to replace part of their
SW–HW integration test matrix, and wants to know what actually backs that
claim.

Pilot scope: MODBUS RTU only, timing clauses only (`RTU-T35-FRAME`,
`RTU-T15-INTRA`, `RTU-MASTER-WAIT`, `RTU-RESP-TIME`). CRC/framing/PDU
clauses are boolean, not timing envelopes, so they have no hull to contain.

## 1. The gap this closes

StableHAL's separate-testing argument has two halves:

* **SW ↔ StableHAL**: drive the real driver against `stablehal-sim`'s DST
  loop across many seeds. If it survives, the software handles the whole
  timing envelope the *simulator* explored.
* **StableHAL ↔ HW**: capture real traffic (SHC1) and judge it against the
  same contract oracle (`RtuContract`). If it's `CONFORMING`, the hardware
  never violates the contract.

Both halves can pass and the composition can still be unsound: nothing
proves the simulator ever exercised the timing region the hardware actually
occupies. A slave that always responds in 1ms is "hardware-conforming", but
if your driver was only ever tested with 1–5ms response jitter, a real
deployment where the device sometimes takes 45ms (still legal — under
`response_timeout`) is untested territory wearing a passing badge.

**Envelope containment makes this checkable, not assumed:**

```
measured HW hull  ⊆  sim-explored hull  ⊆  contract envelope
```

* `measured ⊆ contract` is what a `CONFORMING` verdict already gives you.
* `measured ⊆ sim hull` is the new check — proof that the software was
  actually tested where the hardware actually lives.

## 2. Where the measurements come from

`Contract::step_measured` (defaulted to a no-op sink, so existing contracts
compile unchanged) lets a contract report every timing duration it
computes — passing or not — instead of only the ones that breach:

```rust
fn step_measured(
    &self,
    state: &mut Self::State,
    at: Nanos,
    event: &Self::Event,
    sink: &mut dyn FnMut(Measurement),
) -> Result<(), Breach> { ... }
```

`RtuContract` implements this; `step` delegates to it with a no-op sink, so
there is exactly one judging code path — measurement is a side channel of
judging, never a second implementation of the bounds (ADR-11).
`stablehal_contract::oracle::check_trace_measured` mirrors `check_trace` but
also returns an `EnvelopeAccumulator<N>`: a fixed-capacity, `no_std`,
per-clause fold of `min..=max` and sample count.

## 3. Building the two hulls

**Sim-explored hull** — run the reference master against the reference
slave over many seeds, folding every seed's measurements together:

```
stablehal-certify sim-envelope --seeds 50 --json sim.json
```

Internally this is `stablehal_conformance::runner::sim_envelope(&cfg)`,
which drives a fixed read/write/readback script and maps the sim's wire
trace to `RtuEvent`s the same way `rtu_observer` does, then judges it with
`check_trace_measured`.

**Hardware-measured hull** — parse one or more SHC1 captures, judge each
against the contract, and fold the (only-if-conforming) measurements:

```
stablehal-certify measure capture1.shc1 capture2.shc1 --timeout-ms 50 --json hw.json
```

A capture that doesn't conform aborts the run — an envelope built from a
non-conforming capture would misrepresent what "hardware conforms" covers.

Both commands write the same `stablehal-envelope-v1` JSON:

```json
{
  "format": "stablehal-envelope-v1",
  "contract": "modbus-rtu",
  "source": "sim",
  "traces": 50,
  "clauses": [
    { "id": "RTU-T35-FRAME", "min_ns": 4010415, "max_ns": 5059911, "samples": 120 }
  ]
}
```

## 4. Checking containment

```
stablehal-certify contain --hw hw.json --sim sim.json --json cert.json
```

Per clause, one of three verdicts:

* **COVERED** — the hardware hull sits inside the sim-explored hull.
* **UNCOVERED { gap_low_ns, gap_high_ns }** — hardware operated outside what
  the simulator ever exercised, by this many nanoseconds on the low/high
  side. This is a real finding: widen the sim's jitter/response-delay
  configuration (or add seeds) until it does cover the observed region,
  then re-test the software against the wider envelope.
* **UNPROBED** — the hardware envelope has samples for a clause the sim
  envelope has none for (a hull merge problem, or a clause the DST script
  never exercises).

Exit code is `SUCCESS` iff every measured clause is `COVERED`. The emitted
`stablehal-certificate-v2` embeds both hulls per clause plus the verdict, so
a CI failure is self-explanatory without re-running anything:

```json
{
  "format": "stablehal-certificate-v2",
  "contract": "modbus-rtu",
  "hw_traces": 3,
  "sim_traces": 50,
  "verdict": "UNCOVERED",
  "clauses": [
    { "id": "RTU-RESP-TIME",
      "hw":  { "min_ns": 5058893, "max_ns": 20059911, "samples": 3 },
      "sim": { "min_ns": 5012714, "max_ns": 5059911,  "samples": 60 },
      "verdict": { "status": "UNCOVERED", "gap_low_ns": 0, "gap_high_ns": 15000000 } }
  ]
}
```

## 5. What this does and doesn't prove

* It proves the software was tested against a timing region that contains
  every value the hardware was actually observed to produce — for the
  timing clauses the contract currently measures.
* It does **not** prove the hardware will never exceed that region in the
  future (more captures narrow the risk, they can't eliminate it — this is
  an observed hull, not a formal bound).
* It is min/max only, not a statistical/percentile claim — see
  `next_steps.md` / architecture.md risk log for planned extensions.
* Non-timing clauses (CRC, framing, PDU form, response identity) are
  boolean pass/fail; containment doesn't apply to them, `CONFORMING` already
  covers them completely.
