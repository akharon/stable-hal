# User Guide — CAN data-link contract (`stablehal-can`)

Audience: driver developers writing protocol code against the StableHAL CAN
boundary, and implementers providing a CAN backend (simulator or hardware).

v1 scope: **Classic CAN 2.0B** (standard + extended identifiers), data
frames, remote frames, error frames, judged at **frame granularity** — the
granularity software contracts actually live at (controllers handle
stuffing/CRC in hardware; drivers see frames). CAN FD is deferred to v2.

## 1. CAN has no master/slave — read this before anything else

Every other StableHAL pilot (MODBUS, I2C, LIN, SPI) is a master-plus-
addressed-slaves bus: one party polls, others answer. CAN is a broadcast bus:
**any node can transmit, and every other node that correctly receives a
frame drives the dominant ACK bit — regardless of whether it cares about
that frame's id.** There is no "not for me" response in real CAN; only
"I didn't receive it correctly" (which surfaces as no ACK, a wire-contract
violation if it happens).

`stablehal_can::node::CanNodeActor` models that directly instead of
mirroring the other protocols' actor/device split:

```rust
pub trait CanNodeActor {
    fn poll_tx(&mut self, now: Nanos) -> Option<CanFrame>;
    fn on_sent(&mut self) {}                     // this node's frame just won arbitration
    fn on_frame(&mut self, frame: &CanFrame) -> bool; // another node's frame arrived; ack it?
    fn done(&self) -> bool { false }              // default: never done (passive/responder)
}
```

`CanWorld` polls every registered node's `poll_tx` each bus-idle tick, picks
the arbitration winner (lowest `CanId` — `CanId`'s derived `Ord` ranks
`Standard` before `Extended` then by numeric value, matching real CAN
priority; a documented simplification of bit-level arbitration, since the
reference topology of one tester + one responder rarely contends in
practice), calls `on_sent()` on the winner and `on_frame()` on every other
node, then emits one `CanEvent::Frame` with the OR of every other node's ACK.

## 2. Raw CAN has no request/response — that's an application convention

`CanRegisterNode` (the reference responder) replies to a fixed `request_id`
with `response_id` carrying register bytes (first data byte = pointer, same
idiom as `RegisterPointerDevice`/`SpiRegisterDevice`). **This pattern is not
part of `CanContract`** — it's an application-level convention this
reference node layers on top of the raw data-link contract, the same way
MODBUS's function codes sit on top of the UART line contract. Don't cite it
as if it were a CAN spec requirement; a real CAN application is free to use
any convention (or none).

## 3. Configuration

```rust
use stablehal_can::CanConfig;

let config = CanConfig::default_500k(); // 500 kbit/s, extended ids allowed
let config = CanConfig::default_1m();   // 1 Mbit/s, extended ids allowed
```

## 4. Reference nodes

```rust
use stablehal_can::node::{CanNodeBugs, CanRegisterNode, CanScriptStep, CanScriptedNode};
use stablehal_can::CanId;

const REQUEST_ID: CanId = CanId::Standard(0x100);
const RESPONSE_ID: CanId = CanId::Standard(0x101);

const SCRIPT: &[CanScriptStep] = &[
    CanScriptStep { id: REQUEST_ID, rtr: false, data: &[0x10] }, // read register 0x10
];
let tester = CanScriptedNode::new(SCRIPT, RESPONSE_ID);

let mut responder = CanRegisterNode::new(REQUEST_ID, RESPONSE_ID);
responder.write_reg(0x10, 0xAB);
```

`CanNodeBugs` has three fields:

| Bug | Wire-contract breach? | Why |
|---|---|---|
| `silent_no_ack` | **Yes** — `CAN-ACK` | On this reference 2-node bus the responder is the *sole* other node, so its ACK is the only possible one. |
| `wrong_response_data` | No | Data-correctness bug, mirrors `I2cSlaveBugs::wrong_read_data`. |
| `silent_no_response` | No | The request/response convention is application-level (§2), not part of `CanContract` — a responder that never replies still conforms on the wire. |

## 5. The CAN contract (what a party signs)

Judged per-`Frame`-event atomically — no cross-event state, unlike the
byte/character-level contracts in the other four pilots.

| Clause | Checked how | Closed-loop tested? |
|---|---|---|
| `CAN-ID-FIELD` | Every `Frame` event: id must fit the configured width. | Yes |
| `CAN-RTR-DISC` | Every `Frame` event: a remote frame (`rtr: true`) must carry zero data bytes. | Yes |
| `CAN-ACK` | Every `Frame` event: `acked` must be `true`. | Yes |
| `CAN-FRAME-START` | **Never checked in `step`** — its own code comment says SOF presence is "assumed for any valid frame observation." | No |
| `CAN-EOF` | Only via a `FormError` event. | No — this reference world never emits one. |
| `CAN-CRC` | Only via a `CrcError` event. | No — this reference world never emits one. |
| `CAN-ERROR-FRAME` | Only via an `ErrorFrame` event. | No — this reference world never emits one. |
| `CAN-OVERLOAD` | Only via an `OverloadFrame` event. | No — this reference world never emits one. |
| `CAN-BIT-STUFFING` | Not checked — requires bit-level observation, out of scope at frame granularity. | No |

The last five all need a genuine bus-monitor-level observer (something
watching individual bits, not just assembled frames) to ever fire — a real
gap for anyone wiring hardware capture against this contract, tracked
alongside on-target bring-up (`next_steps.md` item 1).

## 6. Simulating a bus: `CanWorld`

```rust
use stablehal_can::node::{CanRegisterNode, CanScriptStep, CanScriptedNode};
use stablehal_can::{CanConfig, CanId};
use stablehal_sim::can::{CanWorld, CanWorldConfig};
use stablehal_contract::Nanos;

const REQUEST_ID: CanId = CanId::Standard(0x100);
const RESPONSE_ID: CanId = CanId::Standard(0x101);
const SCRIPT: &[CanScriptStep] = &[CanScriptStep { id: REQUEST_ID, rtr: false, data: &[0x10] }];

let mut world = CanWorld::new(CanWorldConfig::new(/* seed */ 42, CanConfig::default_500k()));
world.add_node(Box::new(CanScriptedNode::new(SCRIPT, RESPONSE_ID)));
world.add_node(Box::new(CanRegisterNode::new(REQUEST_ID, RESPONSE_ID)));
let summary = world.run_until(Nanos::from_millis(50)).unwrap();
println!("hash {:016x}", world.trace_hash());
```

Frame duration is an approximated bit-count estimate (SOF + id + control +
data + CRC + ACK + EOF + IFS, +20% margin for worst-case bit stuffing since
exact stuffed length isn't modeled at this granularity), jittered from the
seeded stream so seed sweeps explore distinct schedules. `run_until` stops
once *any* registered node reports `done()` — correct for one tester + one
passive responder; a bus with more than one actively-driving node would need
a richer termination rule (`next_steps.md`).

## 7. Conformance suite

`stablehal_conformance::can::STANDARD_SUITE` (behind the `runner` feature)
exercises the reference responder with three cases: `CAN-DATA-FRAME-BASIC`,
`CAN-REMOTE-FRAME`, `CAN-EXTENDED-ID`, covering `CAN-ID-FIELD`,
`CAN-RTR-DISC`, `CAN-ACK` — the only clauses reachable through a
`CanEvent::Frame` observation in a conforming run (see §5 for the rest).
