# StableHAL — Design notes (forward-looking, not scheduled)

Design explorations that are **not on the committed backlog** ([next_steps.md](../next_steps.md))
and **not implemented**. They are recorded here so the reasoning survives; each
would need its own design + sign-off before any code. Nothing here changes the
current contract semantics or golden values.

---

## 1. Record-and-replay for SW testing: where it is sound, and where state breaks it

A natural second use case falls out of the normal order of work (hardware is
built and tested first; StableHAL then generates inputs and observes the DUT):
sit on the wire, **capture the actual hardware response**, and replay those
captured request→response pairs as golden vectors to test the software driver
with no hardware attached — essentially automatic mock/stub generation from
certified traffic, composing cleanly with the existing SHC1 capture path. It is
real and valuable, but its validity is bounded, and the bound is **statefulness**.

**Two places response values could live.** In StableHAL as built, the *contract
clauses* govern **wire form and timing** (framing, CRC/PID parity,
chip-select/mode discipline, response-time envelopes) — **not data values**.
The value a register returns is produced by the **reference device model**
(`RegisterBank`, the register-pointer devices, the CAN register node), and data
correctness is checked by the *actor's* property assertion in the DST loop
("0 silent data corruptions"), not by the oracle. So "the response is in the
contract" holds for *structure* and *timing* but not, in general, for *payload*.
Record-and-replay is precisely the technique that fills that payload gap when no
reference model exists — which is why its statefulness limit is the crux.

**Layer 1 — the wire/data-link contracts are (almost) stateless per
transaction,** so capture-replay of framing is safe: the line-phase FSMs reset
at each frame boundary. The one exception among the supported protocols is
**MODBUS**, whose contract carries an explicit cross-transaction overlay
(`Proto::ExpectRequest ⇄ AwaitResponse { unit, fc, request_end }` in
[contract.rs](../crates/stablehal-modbus/src/contract.rs)) — a response is only
legal while a matching request is outstanding. I2C, SPI, and CAN have no such
coupling at the wire layer.

**Layer 2 — the application/device layer is stateful in all four,** and that is
what breaks naive replay:

| Protocol | Stateful device element | Why a captured (req → resp) pair is only conditionally valid |
|---|---|---|
| MODBUS | `RegisterBank` + `Proto` coupling | An FC03 read reflects earlier FC06/FC10 writes; a response is legal only against an outstanding request. |
| I2C | register-pointer device (auto-incrementing pointer + 256-byte store) | A read returns bytes from the current pointer, which prior writes/reads moved. |
| SPI | same first-byte-is-pointer idiom | Value returned depends on the pointer and register contents set by earlier transfers. |
| CAN | register node (`regs: [u8; 256]` + pointer + pending-frame state) | A response frame's payload depends on prior register writes and which frame is pending. |

**Conclusion.** Capture-and-replay is **sound for stateless or idempotent
exchanges** (pure reads of unchanging registers, structural/timing conformance,
or any exchange where the device is reset to a known state before each replayed
request) — there the response *is* a pure function of the request. It is
**unsound as a blind lookup table** the moment the device keeps state: a stored
`(request → response)` pair is valid only for the specific state trajectory
under which it was captured. Crucially, that statefulness lives in the
**application layer, not the transport** — so it is not determined by which of
SPI/I2C/MODBUS/CAN you chose, but by the application protocol built on top.

The productizable form is therefore **not** a flat captured lookup table but a
**captured state-machine / model** — replay against a device model whose state
is advanced by the same request stream. StableHAL already has the right
primitive: its reference devices *are* such models. The honest framing of a
"capture responses for SW testing" feature is "capture enough to *build or
validate* the device model," not "capture pairs and replay them blindly."

---

## 2. Make the contract declarative, typed, and layerable (for LLM authoring + application-protocol reach)

Two forces point the same way. (a) The natural authoring workflow is "read the
spec, emit the contract," and that author is increasingly an LLM — so the
interface should let a model emit *structured data it can keep consistent with a
citation*, not a bespoke imperative state machine it must get right line by
line. (b) The reach the project needs is **application protocols layered on top
of CAN/I2C/SPI/MODBUS** (register maps, mode/config semantics, request/response
coupling — the stateful layer in note 1), and today those live in ad-hoc
reference-device Rust, invisible to the contract vocabulary.

Where the current interface pushes against both
([oracle.rs](../crates/stablehal-contract/src/oracle.rs),
[clause.rs](../crates/stablehal-contract/src/clause.rs)):

- `Clause` carries only `id`/`citation`/`text` — it is documentation; the
  machine-checkable obligation lives separately inside imperative `step`. The
  author must hand-maintain the coupling. G3 asks for it; nothing structural
  guarantees it.
- `type Event` is a bespoke Rust enum per protocol (`LineEvent`, `I2cEvent`, …)
  and `step` is a hand-written match. Every protocol reinvents the shape; none
  is introspectable or generatable uniformly.
- There is no layering primitive: an application contract cannot say "in terms
  of the transport's finalized frames, this response payload must equal
  `f(device_state)`." Payload-as-function-of-state has no home, so note 1's
  statefulness cannot be expressed as a clause.

**Sketch (additive; keeps every existing contract compiling; G1/G2 intact):**

1. **Turn each `Clause` into an executable, typed obligation.** Add a
   declarative `obligation` with a small closed vocabulary — `Timing { bound }`,
   `Ordering { after, before }`, `Field { selector, predicate }`,
   `Response { of, payload_eq: StateFn }`, `Guard { when, forbid }` — and a
   generic engine that interprets it. Imperative `step` becomes the escape
   hatch, not the default. The model emits one artifact (clause = citation +
   obligation), self-consistent by construction.
2. **A common, self-describing event model** (`FrameStart` / `Field{name,bytes}`
   / `FrameEnd` / `Idle` / `Fault{kind}` + a protocol tag) the declarative
   obligations pattern against generically, alongside the existing `type Event`
   for hand-written contracts.
3. **First-class application-over-transport layering:** let a contract declare a
   transport contract as a dependency and attach a typed, bounded `StateModel`
   (a `key → value` store with declarative transition rules — the generic form
   of `RegisterBank` / the register-pointer devices), so "an FC03 read returns
   `bank[addr]` reflecting prior writes" is a `Response` clause over a
   `StateModel`, not buried Rust.
4. **A canonical serialized schema** `stablehal-contract-v1` — hand-rolled
   JSON/TOML mirroring the existing `stablehal-envelope-v1` (still zero-dep) —
   so "LLM reads spec → emits contract" is file generation validated against a
   schema, round-trippable and diffable in review.
5. **One explicit, documented surface for authors:** a single prelude, the
   schema doc, and two or three worked spec→contract examples.

**Trade-offs, honestly.** The declarative vocabulary must stay a *closed* set to
preserve determinism and auditability (G2) — anything it cannot express falls
back to imperative `step`, and that fallback line is where LLM authoring stops
being safe, so the vocabulary's coverage of real spec language is the whole bet.
The `StateModel` reintroduces cross-transaction state into the kernel that was
deliberately kept in reference devices; it must be declarative and bounded
(fixed-capacity, no alloc on the `no_std` path) or it erodes the determinism
guarantees that are StableHAL's core asset.
