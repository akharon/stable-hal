# StableHAL — IPC channel contract

`stablehal-ipc` applies StableHAL's contract methodology to **inter-process
communication**: a request/response datagram exchange over a bounded queue
(the Unix-domain-socket / POSIX message-queue / shared-ring shape), local or
cross-host.

This is the first non-wire pilot in the workspace, so §1 explains why it
belongs here at all before the API walkthrough starts.

---

## 1. Why IPC is a contract problem

The five wire pilots (MODBUS RTU, I2C, LIN, SPI, CAN) share a shape: a
physical medium, timing derived from a crystal, and nondeterminism that is
electrical. IPC between two real-time-scheduled processes on one host has
almost none of that — which is exactly why IPC is usually tested with a
single happy-path integration test and then trusted.

Two deployments break that assumption, and they are the common ones:

**Non-real-time scheduling.** A responder under `SCHED_OTHER` does not answer
in a fixed time; it answers when the scheduler next runs it. On a loaded box
that is anywhere from ~10 µs to tens of milliseconds. That is a genuine
`min..=max` envelope — three orders of magnitude wide — and it is the single
most common source of "worked in CI, timed out in production".

**Cross-host transport.** Once the channel leaves the machine, delivery
latency becomes an envelope and the fault vocabulary grows terms a local pipe
never has: duplicate delivery and reordering.

Both are *bounded nondeterminism*, so both are contractualizable exactly as
[architecture.md](architecture.md) §4 describes: state the envelope, close the
fault set, sample it under a seed, judge it with one oracle.

### What is genuinely different from the wire pilots

| Concern | Wire buses | IPC |
|---|---|---|
| Timing origin | crystal / baud rate | OS scheduler + transport |
| Envelope width | ±2–5% | up to 2000× (`10µs..=20ms`) |
| Fault vocabulary | framing, parity, noise, overrun | peer death, backpressure, truncation, duplicate, reorder |
| Endpoint liveness | a device is always "there" | a peer can **die mid-transaction**, permanently |
| Flow control | none (or clock stretching) | bounded queue ⇒ backpressure is a first-class *legal* outcome |

The last two rows have no analogue in any wire pilot. A MODBUS slave cannot
cease to exist mid-frame, and a SPI master is never told "try again later".
Those two facts are why this is a new contract rather than a
re-parameterization of an existing one — and they are where the bugs live.

### Granularity

Modeled at **datagram** granularity: one whole message is one event. This is
the level at which IPC software contracts live — a caller sends a request and
awaits a response; it never observes partial frames the way a UART driver
does. Truncation is therefore an explicit fault term, not a byte-level
framing state.

---

## 2. Quick start

```rust
use stablehal_ipc::{IpcConfig, IpcResponderDevice, IpcStep, ScriptedRequester};
use stablehal_sim::ipc::{IpcRequesterAdapter, IpcWorld, IpcWorldConfig};
use stablehal_contract::Nanos;

const SCRIPT: &[IpcStep] = &[
    IpcStep { id: 1, payload: &[0xAB, 0xCD] },
    IpcStep { id: 2, payload: &[0x11, 0x22] },
];

let mut world = IpcWorld::new(
    IpcWorldConfig::new(0x5EED, IpcConfig::local_non_realtime()),
    Box::new(IpcRequesterAdapter::new(ScriptedRequester::new(SCRIPT))),
    Box::new(IpcResponderDevice::new()),
);

let summary = world.run_until(Nanos::from_millis(500)).unwrap();
assert!(summary.requester_done);
println!("trace hash: {:x}", world.trace_hash());
```

Every run is a pure function of the seed: same seed ⇒ same trace hash, byte
for byte (`crates/stablehal-sim/tests/ipc_dst.rs::seed_replay_is_byte_exact`).

---

## 3. Deployment profiles

Three profiles ship, differing only in their envelopes and `ChannelKind`.
The same contract, drivers, and oracle serve all three.

| Profile | Delivery | Response | Reorder/dup possible |
|---|---|---|---|
| `IpcConfig::local_non_realtime()` | 200 ns..=5 µs | **10 µs..=20 ms** | no |
| `IpcConfig::local_realtime()` | 200 ns..=2 µs | 5 µs..=200 µs | no |
| `IpcConfig::cross_host()` | 50 µs..=5 ms | 10 µs..=20 ms | **yes** |

`local_realtime` exists so a deployment that genuinely promises tight
turnaround can state that and be held to it. Running the same code under
`local_non_realtime` and `local_realtime` is the cheapest way to find out
whether your service actually depends on being scheduled promptly.

`ChannelKind` gates the ordering and duplicate clauses. A local kernel queue
cannot reorder or duplicate, so those clauses are **not judged** on
`LocalQueue` — claiming to have tested them locally would be a false coverage
claim. Conversely, a `Duplicate`/`Reordered` fault reported on a `LocalQueue`
is treated as a *model* error and flagged, because the transport cannot
actually do that.

---

## 4. Clause table

| Clause | Citation | Obligation |
|--------|----------|------------|
| `IPC-SEQ` | §1 | Request/response strictly alternate per correlation id; a response requires a matching outstanding request. |
| `IPC-RESP-TIME` | §2 | A responder answers within the scheduling-latency envelope. |
| `IPC-DELIVERY` | §3 | Delivery latency (send → receive) stays inside the transport envelope. |
| `IPC-CORRELATION` | §4 | A response carries the correlation id of the request it answers. |
| `IPC-MSG-MAXLEN` | §5 | No datagram exceeds the configured maximum length. |
| `IPC-BACKPRESSURE` | §6 | A send into a full queue must fail loudly (`WouldBlock`), never silently drop. |
| `IPC-NO-DUPLICATE` | §7 | A correlation id is answered at most once. |
| `IPC-PEER-DEATH` | §8 | Peer termination surfaces as an explicit `PeerGone` fault; no traffic is legal after peer exit. |
| `IPC-ORDERING` | §9 | Datagrams within one correlation stream are observed in send order (cross-host only). |

Citations are to this document's own numbered obligations rather than to an
external standard: unlike MODBUS or LIN, there is no single normative IPC
spec to cite. That is stated plainly rather than papered over with a
plausible-looking reference — the clause text *is* the specification here,
and a deployment adopting it is agreeing to these nine obligations.

### Timing clauses and containment

`IPC-DELIVERY` and `IPC-RESP-TIME` are measured through `step_measured`
(ADR-11), so they participate in envelope containment. See §7.

---

## 5. Closed fault vocabulary

```rust
pub enum IpcFault {
    PeerGone,    // terminal: peer exited
    WouldBlock,  // queue full — legal and recoverable
    Truncated,   // short read against a bounded buffer
    Duplicate,   // cross-host only
    Reordered,   // cross-host only
}
```

`WouldBlock` is the notable one: it is a fault the environment injects that a
conforming endpoint must **survive**, not avoid. The reference requester
retries up to `MAX_SEND_ATTEMPTS` and then records an honest `GaveUp`
outcome — a *result*, not a violation, exactly as architecture.md §8.3
distinguishes them.

`PeerGone` is the only terminal fault (`IpcFault::is_terminal()`).

---

## 6. Reference implementations and the bug catalog

* **`ScriptedRequester`** — drives a fixed script, one request at a time.
  The one structurally new behavior versus the wire reference drivers: it
  must handle backpressure. A wire master owns the bus and can always
  transmit; an IPC requester can be told "try later" at any moment.
* **`IpcResponderDevice`** — echoes the request payload with each byte
  incremented, under the request's correlation id.

Unlike SPI — where a slave has no hook to misbehave on wire-level clauses —
an IPC responder controls almost everything it is judged on. The bug catalog
is correspondingly rich, and each entry maps to exactly one clause (G3):

| Bug | Caught by |
|---|---|
| `wrong_correlation` | `IPC-CORRELATION` |
| `never_answer` | `IPC-RESP-TIME` (at end of observation — the silent-hang mode) |
| `answer_twice` | `IPC-NO-DUPLICATE` |
| `oversize_response` | `IPC-MSG-MAXLEN` |
| `slow_response` | `IPC-RESP-TIME` |
| `wrong_payload` | **nothing** — see below |

`wrong_payload` is deliberately *not* a channel-contract breach, the same
honest distinction `I2cSlaveBugs::wrong_read_data` and
`SpiSlaveBugs::wrong_read_data` draw. `IpcEvent` carries correlation id and
length, never payload bytes, because payload correctness is an application
obligation, not a channel one. The DST test asserts **both** halves: the
trace hash is identical between the buggy and conforming runs, *and* the
property check on `IpcWorld::requester_outcomes()` catches it
(`ipc_dst.rs::wrong_payload_bug_is_caught_by_the_property_check`).

---

## 7. Envelope containment

Containment carries more weight for IPC than for any wire pilot. A wire bus's
timing hull is narrow and predictable, so "did the sim cover the hardware
region?" is nearly always yes. A non-real-time IPC channel's hull spans
orders of magnitude and depends on machine load, so the question is genuinely
open — and answering it is the difference between "our tests pass" and "our
tests pass *where production actually runs*".

```rust
use stablehal_conformance::ipc::runner::sim_envelope_sweep;

let (result, hull) = sim_envelope_sweep(50);
assert!(result.is_ok());
println!("{}", hull.hull_for("IPC-RESP-TIME").unwrap());
```

`sim_envelope_sweep(n)` folds `n` seeds' hulls into one region — the right
comparison target, since a single seed explores one point per clause, not a
region.

`tests/ipc_envelope.rs` proves the check has teeth: a "production" hull whose
latencies run slower than anything the sweep explored — still
contract-conforming — is correctly reported as **not** contained. That is the
real-world case: production runs on a loaded box, CI does not, and both pass
their own contract separately.

---

## 8. Conformance suite

`stablehal_conformance::ipc::STANDARD_SUITE` — 5 cases, 3 positive and 2
negative:

| Case | Probes | Expects |
|---|---|---|
| `IPC-BASIC-EXCHANGE` | SEQ, CORRELATION, DELIVERY | pass |
| `IPC-SCHEDULING-ENVELOPE` | RESP-TIME | pass |
| `IPC-BACKPRESSURE-RETRY` | BACKPRESSURE | pass |
| `IPC-WRONG-CORRELATION` | CORRELATION | breach of `IPC-CORRELATION` |
| `IPC-SILENT-HANG` | RESP-TIME, PEER-DEATH | breach of `IPC-RESP-TIME` |

Each case declares `expect_breach` explicitly. A negative case whose bug
stops firing is reported as a **failure**, not a pass — that is what stops
the suite rotting into a no-op, and it is itself tested
(`ipc_closed_loop.rs::a_negative_case_that_stops_failing_is_reported_as_a_failure`).

---

## 9. Known limits

* **Hardware/software-certification path exists, but is local-host-only.**
  §10 covers the real-`AF_UNIX`-socket capture path. It handles exactly one
  requester process and one responder process on one host — cross-host
  capture (which `ChannelKind::CrossHost` models as a contract profile) has
  no capture-side implementation, and neither does multi-requester fan-in.
* **`SilentDrop` is never emitted live** by the real-socket capture port,
  the same honest limit the reference *simulated* world has: a silently
  dropped datagram can only be inferred by absence (an unanswered
  transaction at `EndOfObservation`), never observed directly, because
  nothing exists to observe. Declared, not silently worked around.
* **Single requester, single responder.** No fan-in from multiple client
  processes, and no shared-queue contention between them. The contract's
  per-correlation-id state would extend to it, but neither the reference
  world nor the real-socket capture port model it.
* **`MAX_TRACKED = 16`** concurrent correlation ids in the contract state.
  Exceeding it is reported as `IPC-SEQ` rather than silently forgotten.
* **Containment is min/max only**, not a percentile guarantee — the same
  scope limit ADR-11 states for every protocol. For a scheduler-latency
  envelope, where the distribution is heavily tailed, this limit matters more
  than it does for a wire bus: a hull says where the extremes were, not how
  often.
* `Truncated` is in the fault vocabulary but neither the reference world nor
  the real-socket capture port ever inject it (no partial-datagram path
  exists in either model). Declared, not silently dropped — the same
  honesty pattern as `SPI-DATA-VALID` and `CAN-BIT-STUFFING`.

---

## 10. Capturing a real Unix-socket service

`IpcContract` was always `no_std` and judges any `IpcEvent` trace regardless
of its origin — but until this section's contents existed, nothing produced
one from a real running process pair, so no real IPC service could ever be
certified. This closes that gap for the simplest, most common case: one
requester process and one responder process on one host, talking over a real
`AF_UNIX SOCK_DGRAM` socket.

### 10.1 Why a real-socket port, not an interposer

The obvious approach — an `LD_PRELOAD` interposer hooking `sendmsg`/
`recvmsg` via `dlsym`/`RTLD_NEXT` — was the original plan, since it can
capture *any* existing service without touching its source. But
`stablehal-ipc`'s reference driver (`ScriptedRequester`) and reference
device (`IpcResponderDevice`) are already written purely against the
`IpcRequester`/`IpcResponder` traits: they have no idea whether the port
underneath is `stablehal_sim`'s in-memory queue or a real socket. That makes
a much simpler design possible: implement those same traits over a real
socket (`tools/ipc-capture-shim`'s `RealSocketRequester`/
`RealSocketResponder`), and point the *unmodified* reference driver/device
at it. No `unsafe`, no libc FFI, no interposition — pure `std`. The
trade-off is scope: this captures a service built against
`stablehal-ipc`'s traits, not an arbitrary pre-existing binary. For this
pilot's acceptance bar — proving the capture-to-certify loop closes at all
— that trade was worth it.

### 10.2 Wire framing

`stablehal-ipc` has no wire encoding of its own (the simulated world passes
`Datagram` structs in memory), so the real-socket port defines one: one
`SOCK_DGRAM` datagram carries `id: u32 LE | len: u16 LE | payload: [u8;
len]` (`len <= MAX_PAYLOAD`). `SOCK_DGRAM` preserves message boundaries, so
nothing beyond this is needed.

A responder's socket is *bound*, not *connected* — `AF_UNIX` datagram
sockets have no accept step on the passive side — so it replies via
`send_to` the address a request arrived from. That address is only usable if
the requester **also bound** its own socket before connecting: an
unbound client has no addressable identity, and a responder's `send_to`
against an unnamed peer fails with `EINVAL`. Both fixture binaries
(`ipc-requester-fixture`, `ipc-responder-fixture` in
`tools/ipc-capture-shim/src/bin/`) do this correctly; a hand-written client
must too.

### 10.3 Shared capture origin

Both sides must timestamp against the **same** origin for a merge to
produce a sensible time order. `std::time::Instant` cannot serve this role
even though it is backed by `CLOCK_MONOTONIC` on Linux: safe `std` gives no
way to read its raw value or share it between processes, and one process's
`Instant::now()` only orders against other `Instant`s in that same process.
`Capture::new` therefore takes an explicit `SystemTime` origin, which
whatever spawns both fixtures computes once (e.g. `SystemTime::now()` as
nanoseconds since `UNIX_EPOCH`) and passes to both via the
`IPC_CAPTURE_EPOCH_NS` environment variable. `SystemTime` can in principle
jump on an NTP step, unlike `CLOCK_MONOTONIC` — accepted here in exchange
for staying in safe `std`. This is also why the capture path is
local-host-only: cross-host would need real clock-skew correction, which
this design does not attempt.

### 10.4 Walkthrough

```sh
export IPC_CAPTURE_EPOCH_NS=$(python3 -c 'import time; print(time.time_ns())')

# Responder binds first.
./target/debug/ipc-responder-fixture /tmp/ipc.sock responder.shcipc &
sleep 0.3
./target/debug/ipc-requester-fixture /tmp/ipc.sock requester.shcipc

# Merge the two one-sided captures into one time-ordered trace.
stablehal-certify merge-ipc requester.shcipc responder.shcipc --out merged.shcipc

# Judge it — CONFORMING for a clean exchange.
stablehal-certify judge merged.shcipc
```

`judge`/`measure` dispatch on the capture's own SHC1 header `contract`
field (0x0004 for IPC, §13.3), the same file-format polymorphism the RTU
path already had a slot for. `--profile` selects the `IpcConfig` to judge
against; it defaults to `IpcConfig::real_socket_local()`, **not**
`local_non_realtime()` — a real kernel round trip between two OS processes
routinely costs tens of microseconds even with zero scheduling stall, past
`local_non_realtime`'s in-sim-queue-calibrated `IPC-DELIVERY` bound
(`200ns..=5µs`). Judging a real capture against that bound would flag
ordinary, correct behavior as a breach; `real_socket_local` widens
`IPC-DELIVERY` to `200ns..=500µs` while leaving `resp_time` unchanged, since
the scheduling-latency envelope this pilot cares about doesn't depend on
which profile measured the wire itself.

### 10.5 The containment payoff

The reason this pilot cares about a real capture at all is the same reason
containment (§7) was built: does the simulated envelope actually cover
where a real deployment runs?

```sh
stablehal-certify measure merged.shcipc --json hw_envelope.json
stablehal-certify sim-envelope --protocol ipc --seeds 50 --json sim_envelope.json
stablehal-certify contain --hw hw_envelope.json --sim sim_envelope.json
```

Run against a real capture, `IPC-DELIVERY` comes back **UNCOVERED**: a real
kernel round trip between two OS processes is genuinely slower than
anything an in-memory simulated queue can produce. That is not a bug in
either side — it is exactly the fact containment exists to surface, and
`tools/ipc-capture-shim/tests/ipc_containment_from_capture.rs` asserts it
directly rather than leaving it to be discovered by whoever runs the
commands above.

---

## 11. Where the code lives

| Block | File |
|---|---|
| Config, traits, fault vocabulary | [lib.rs](../crates/stablehal-ipc/src/lib.rs) |
| Contract (9 clauses, `step_measured`) | [contract.rs](../crates/stablehal-ipc/src/contract.rs) |
| Reference requester | [requester.rs](../crates/stablehal-ipc/src/requester.rs) |
| Reference responder + bug catalog | [device.rs](../crates/stablehal-ipc/src/device.rs) |
| Simulator world | [ipc.rs](../crates/stablehal-sim/src/ipc.rs) |
| Conformance suite | [ipc/mod.rs](../crates/stablehal-conformance/src/ipc/mod.rs) |
| DST closed loop | [ipc_dst.rs](../crates/stablehal-sim/tests/ipc_dst.rs) |
| Containment (sim) | [ipc_envelope.rs](../crates/stablehal-conformance/tests/ipc_envelope.rs) |
| Capture format (SHC1 IPC record shape) | [stablehal-shc1/src/lib.rs](../crates/stablehal-shc1/src/lib.rs) |
| Real-socket capture port + fixtures | [tools/ipc-capture-shim](../tools/ipc-capture-shim/src/lib.rs) |
| `stablehal-certify` IPC dispatch + `merge-ipc` | [judge.rs](../tools/stablehal-certify/src/judge.rs), [measure.rs](../tools/stablehal-certify/src/measure.rs), [merge_ipc.rs](../tools/stablehal-certify/src/merge_ipc.rs) |
| Containment from a real capture | [ipc_containment_from_capture.rs](../tools/ipc-capture-shim/tests/ipc_containment_from_capture.rs) |
