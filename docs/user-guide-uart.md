# User Guide — UART HAL (`stablehal-uart`)

Audience: driver developers writing protocol code against the StableHAL UART
boundary, and implementers providing a UART backend (simulator or hardware).

## 1. The two faces of the crate

| Face | Type | Who uses it |
|---|---|---|
| SW-facing HAL | `UartBus` trait | protocol drivers (e.g. `RtuMaster`) |
| Wire-facing contract | `LineContract` (implements `Contract`) | oracles, simulators, hardware observers |

Both are `no_std`. A full-duplex link is modeled as **two independent simplex
lines**, each with its own `LineContract` instance and its own transmitter.

## 2. Configuration

```rust
use stablehal_uart::{UartConfig, Parity};

// MODBUS RTU default framing: 8E1 → 11 bits per character on the wire.
let cfg = UartConfig::rtu_default(9600);
assert_eq!(cfg.bits_per_char(), 11);
let ct = cfg.char_time(); // 1_145_833 ns at 9600 baud — integer, truncated

// Custom framing:
let cfg = UartConfig { baud: 115_200, data_bits: 8, parity: Parity::None, stop_bits: 1 };
```

**Rule:** every component — driver, simulator, oracle, conformance generator —
must derive character timing through `UartConfig::char_time()`. Integer
truncation is part of the contract; two components computing the "same" bound
differently by one nanosecond is a bug class this rule eliminates.

## 3. Writing a driver against `UartBus`

```rust
use stablehal_contract::Nanos;
use stablehal_uart::{UartBus, UartError};

fn poll_my_driver<B: UartBus>(now: Nanos, bus: &mut B) {
    // Receive: drain everything that arrived. Err(..) is a *contract-legal*
    // fault (Framing/Parity/Noise/Overrun) — your driver must handle it.
    while let Some(item) = bus.try_read() {
        match item {
            Ok(byte) => { /* accumulate */ }
            Err(UartError::Framing | UartError::Parity | UartError::Noise) => {
                /* mark current frame poisoned; recover per your protocol */
            }
            Err(UartError::Overrun) => { /* ditto */ }
        }
    }
    // Transmit: single-depth TX. try_write fails while the previous
    // character is still being shifted out — no hidden FIFO, so your
    // driver's pacing is observable and judgeable.
    let _ = bus.try_write(0x42);
}
```

Design conventions (all reference drivers follow them):

* **Poll-driven, no blocking.** Your driver is a state machine
  `poll(now, bus) -> progress`, where progress carries a `wake_at:
  Option<Nanos>` hint (`None` = "only RX can advance me"). This exact shape is
  what [stablehal-sim's Actor](../crates/stablehal-sim/src/lib.rs) adapts, and
  what an ISR-driven target loop will call.
* **Pace your own bytes.** After a successful `try_write` at `t`, the next
  character can start at `t + char_time`. Track it yourself (see
  `RtuMaster`'s `Sending` state) instead of spinning on `TxBusy`.
* **Never touch a clock.** Use only the `now` passed into `poll`.

## 4. The line contract (what a transmitter signs)

| Clause | Obligation |
|---|---|
| `UART-NO-OVERLAP` | Don't start a character while one is on the line. |
| `UART-CHAR-TIME` | Each character occupies exactly `bits_per_char/baud`, within the tolerance envelope (default ±2%). |
| `UART-TX-PAIRING` | Every `TxEnd` terminates the latest `TxStart` with the same byte. |

Wire events: `TxStart{byte}` / `TxEnd{byte}` are transmitter truth (judged);
`Delivered{byte}` / `DeliveredCorrupt{error}` are environment outcomes
(recorded, never blamed on an endpoint).

Judging a trace yourself:

```rust
use stablehal_contract::{Oracle, Nanos};
use stablehal_uart::{LineContract, LineEvent, UartConfig};

let cfg = UartConfig::rtu_default(9600);
let mut oracle = Oracle::new(LineContract::new(cfg));
let t0 = Nanos::from_millis(1);
oracle.observe(t0, &LineEvent::TxStart { byte: 0x01 }).unwrap();
match oracle.observe(t0 + cfg.char_time(), &LineEvent::TxEnd { byte: 0x01 }) {
    Ok(()) => {}
    Err(v) => println!("{v}"), // names the clause, cites the spec, shows measured vs required
}
```

`LineContract::new` sets `char_time_tolerance` to 2% of nominal; tighten or
relax it explicitly if your clock budget differs — it is a contract term, not
an implementation detail.

## 5. The fault vocabulary is closed

`UartError::{Framing, Parity, Noise, Overrun}` is the *complete* set of
receive-side faults. Consequences:

* The simulator injects only these (see `SimConfig::corrupt_ppm`).
* A conforming backend must map every hardware condition into this set.
* A driver that survives DST has, by construction, handled every member.

If a new fault kind is ever needed, that is a **contract change**: add the
variant, bump the contract version, and every artifact (sim, oracle,
conformance) updates together.

## 6. Implementing a backend (checklist)

To provide `UartBus` on new infrastructure (RTOS, another simulator, real
silicon):

1. `try_write` must reject (return `TxBusy`) while a character is in flight —
   single-depth semantics.
2. Timestamp receive items as accurately as your platform allows; deliver
   them in order through `try_read`.
3. Map hardware error flags onto the closed `UartError` set.
4. If you can observe the physical wire (or your own TX shifter), emit
   `TxStart`/`TxEnd` events into an `Oracle<LineContract>` — that is what
   makes your backend *certifiable* rather than merely plausible.
