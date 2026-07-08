//! The I2C wire contract: sequencing, timing, and ACK discipline, expressed
//! as an executable state machine over byte-level bus events (UM10204 rev 7;
//! stretch cap follows the SMBus 3.x convention).
//!
//! The same contract instance judges simulated buses and hardware captures.
//! Each party can be excluded from judgement (`judge_master`/`judge_slaves`)
//! so a conformance tester may deliberately emit malformed stimuli while the
//! device under test is still held to every clause.

use core::mem;

use stablehal_contract::{Breach, Clause, Contract, Nanos, TimeBound};

use crate::timing::I2cTiming;

/// Byte-level wire events, as observed by an independent bus monitor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum I2cEvent {
    /// START condition (also covers repeated START — the contract does not
    /// distinguish them; both open a new addressed phase).
    Start,
    AddrByte {
        addr7: u8,
        read: bool,
        acked: bool,
    },
    DataByte {
        master_to_slave: bool,
        byte: u8,
        acked: bool,
    },
    /// Slave-attributed cumulative stretch observed inside the preceding
    /// byte (address or data).
    Stretch {
        dur: Nanos,
    },
    Stop,
    /// Appended by the harness after all activity: finalizes an open
    /// transaction so it does not escape judgement.
    EndOfObservation,
}

// --- Clauses (citations: NXP UM10204 rev 7; SMBus 3.x for the stretch cap) -

pub static CL_I2C_SEQ: Clause = Clause {
    id: "I2C-SEQ",
    citation: "UM10204 §3.1",
    text: "Well-formed sequencing: Start -> AddrByte -> DataBytes -> (reStart | Stop); \
           no Stop without an open transaction; an open transaction at \
           EndOfObservation is a breach.",
};
pub static CL_I2C_BUS_FREE: Clause = Clause {
    id: "I2C-BUS-FREE",
    citation: "UM10204 Table 10 (tBUF)",
    text: "At least bus_free_min of idle time between a Stop and the next Start.",
};
pub static CL_I2C_BYTE_TIME: Clause = Clause {
    id: "I2C-BYTE-TIME",
    citation: "UM10204 §6",
    text: "Each byte (9 SCL cycles) fits within 9x the SCL period envelope, plus any \
           observed clock stretch.",
};
pub static CL_I2C_READ_NACK_LAST: Clause = Clause {
    id: "I2C-READ-NACK-LAST",
    citation: "UM10204 §3.1.10",
    text: "The final byte of every Read is NACKed by the master; every earlier byte \
           is ACKed.",
};
pub static CL_I2C_ADDR_ACK: Clause = Clause {
    id: "I2C-ADDR-ACK",
    citation: "deployment contract (device presence)",
    text: "A configured slave ACKs its own address byte.",
};
pub static CL_I2C_STRETCH_MAX: Clause = Clause {
    id: "I2C-STRETCH-MAX",
    citation: "SMBus 3.x (clock low timeout)",
    text: "Cumulative clock stretch per byte does not exceed stretch_max.",
};
pub static CL_I2C_WRITE_ACK: Clause = Clause {
    id: "I2C-WRITE-ACK",
    citation: "device-model term (reference register-pointer device)",
    text: "The reference register-pointer device ACKs all in-range writes.",
};

static I2C_CLAUSES: [&Clause; 7] = [
    &CL_I2C_SEQ,
    &CL_I2C_BUS_FREE,
    &CL_I2C_BYTE_TIME,
    &CL_I2C_READ_NACK_LAST,
    &CL_I2C_ADDR_ACK,
    &CL_I2C_STRETCH_MAX,
    &CL_I2C_WRITE_ACK,
];

/// Maximum number of configured slave addresses tracked by the contract.
/// A fixed bound keeps the contract `no_std`-friendly (ADR-9 style tradeoff).
pub const MAX_CONFIGURED_ADDRS: usize = 16;

#[derive(Copy, Clone, Debug)]
pub struct I2cContract {
    pub timing: I2cTiming,
    /// Judge the master's obligations. Disabled when a conformance tester
    /// deliberately emits malformed stimuli.
    pub judge_master: bool,
    /// Judge slave obligations (the DUT in conformance runs).
    pub judge_slaves: bool,
    /// Addresses with a slave actually configured on the bus — needed to
    /// tell an expected `AddrNack` (nobody home) from an `I2C-ADDR-ACK`
    /// breach (a configured device failed to answer its own address).
    configured_addrs: [Option<u8>; MAX_CONFIGURED_ADDRS],
}

impl I2cContract {
    pub fn new(timing: I2cTiming, configured_addrs: &[u8]) -> Self {
        let mut addrs = [None; MAX_CONFIGURED_ADDRS];
        for (slot, &a) in addrs.iter_mut().zip(configured_addrs.iter()) {
            *slot = Some(a);
        }
        I2cContract {
            timing,
            judge_master: true,
            judge_slaves: true,
            configured_addrs: addrs,
        }
    }

    fn is_configured(&self, addr7: u8) -> bool {
        self.configured_addrs.iter().flatten().any(|&a| a == addr7)
    }

    fn fail_master(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_master {
            Err(breach)
        } else {
            Ok(())
        }
    }
    fn fail_slave(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_slaves {
            Err(breach)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
enum BusPhase {
    /// Idle since `since`; no transaction open. `since: None` means the bus
    /// has never been claimed yet (initial state) — the first-ever Start is
    /// exempt from the bus-free obligation, which is meaningful only as a
    /// gap *since the previous Stop*.
    Free { since: Option<Nanos> },
    /// A transaction is open: Start seen, zero or more bytes exchanged.
    /// `awaiting_ack` is the kind of the byte last emitted, if its ack/data
    /// hasn't been finalized as an `I2cEvent::DataByte`/`AddrByte` yet —
    /// tracked implicitly by the event stream itself (see `step`).
    Open {
        /// Direction/address established by the most recent AddrByte.
        addr7: u8,
        read: bool,
        /// True once at least one AddrByte has been observed for this open
        /// phase (distinguishes "just started" from "mid-transaction").
        addressed: bool,
        /// Index of the next data byte for READ-NACK-LAST bookkeeping and
        /// stretch cap accounting per byte.
        byte_stretch: Nanos,
    },
}

#[derive(Clone, Debug)]
pub struct I2cState {
    phase: BusPhase,
}

impl I2cContract {
    fn open_transaction(&self, state: &mut I2cState, at: Nanos) -> Result<(), Breach> {
        if let BusPhase::Free { since: Some(since) } = state.phase {
            let idle = at.saturating_sub(since);
            if idle < self.timing.bus_free_min {
                self.fail_master(Breach::timing(
                    &CL_I2C_BUS_FREE,
                    idle,
                    TimeBound::at_least(self.timing.bus_free_min),
                ))?;
            }
        }
        // A Start while already Open is a repeated start: legal, simply
        // resets the addressed byte expectation without closing the phase.
        state.phase = BusPhase::Open {
            addr7: 0,
            read: false,
            addressed: false,
            byte_stretch: Nanos::ZERO,
        };
        Ok(())
    }

    fn check_byte_time(&self, dur: Nanos, stretch: Nanos) -> Result<(), Breach> {
        let nominal = I2cTiming::byte_time(self.timing.scl.max);
        let budget = nominal.saturating_add(stretch);
        if dur.as_nanos() > budget.as_nanos() {
            // Attribute to whichever party is under judgement; byte timing
            // is a joint obligation in v1 (no per-party split specified).
            self.fail_master(Breach::timing(
                &CL_I2C_BYTE_TIME,
                dur,
                TimeBound::new(Nanos::ZERO, budget),
            ))?;
        }
        Ok(())
    }
}

impl Contract for I2cContract {
    type State = I2cState;
    type Event = I2cEvent;

    fn initial(&self) -> I2cState {
        I2cState {
            phase: BusPhase::Free { since: None },
        }
    }

    fn step(&self, state: &mut I2cState, at: Nanos, event: &I2cEvent) -> Result<(), Breach> {
        match *event {
            I2cEvent::Start => self.open_transaction(state, at),
            I2cEvent::AddrByte { addr7, read, acked } => {
                match &mut state.phase {
                    BusPhase::Open { .. } => {}
                    BusPhase::Free { .. } => {
                        // Address byte with no preceding Start observed: the
                        // bus monitor never sees this in a well-formed run,
                        // but treat as an implicit Start for robustness.
                        self.open_transaction(state, at)?;
                    }
                }
                if let BusPhase::Open {
                    addr7: a,
                    read: r,
                    addressed,
                    byte_stretch,
                } = &mut state.phase
                {
                    *a = addr7;
                    *r = read;
                    *addressed = true;
                    *byte_stretch = Nanos::ZERO;
                }
                let configured = self.is_configured(addr7);
                if configured && !acked {
                    self.fail_slave(Breach::of(&CL_I2C_ADDR_ACK))?;
                }
                Ok(())
            }
            I2cEvent::DataByte {
                master_to_slave,
                acked,
                ..
            } => {
                if let BusPhase::Open {
                    read,
                    addressed,
                    byte_stretch,
                    ..
                } = &mut state.phase
                {
                    if !*addressed {
                        return self.fail_master(Breach::of(&CL_I2C_SEQ));
                    }
                    *byte_stretch = Nanos::ZERO;
                    if !*read && master_to_slave && !acked {
                        // A write byte NACKed by the slave: contract-visible
                        // only for the reference device (I2C-WRITE-ACK); a
                        // real deployment may legitimately NACK (buffer full
                        // etc.), so this is judged only when configured as
                        // a "must-ack" device via I2C-WRITE-ACK — treated as
                        // a slave obligation here to keep v1 simple.
                        self.fail_slave(Breach::of(&CL_I2C_WRITE_ACK))?;
                    }
                }
                Ok(())
            }
            I2cEvent::Stretch { dur } => {
                if let BusPhase::Open { byte_stretch, .. } = &mut state.phase {
                    *byte_stretch += dur;
                    if byte_stretch.as_nanos() > self.timing.stretch_max.as_nanos() {
                        self.fail_slave(Breach::timing(
                            &CL_I2C_STRETCH_MAX,
                            *byte_stretch,
                            TimeBound::new(Nanos::ZERO, self.timing.stretch_max),
                        ))?;
                    }
                }
                Ok(())
            }
            I2cEvent::Stop => {
                let phase = mem::replace(&mut state.phase, BusPhase::Free { since: Some(at) });
                match phase {
                    BusPhase::Open { .. } => Ok(()),
                    BusPhase::Free { .. } => self.fail_master(Breach::of(&CL_I2C_SEQ)),
                }
            }
            I2cEvent::EndOfObservation => {
                let phase = mem::replace(&mut state.phase, BusPhase::Free { since: Some(at) });
                match phase {
                    BusPhase::Open { .. } => self.fail_master(Breach::of(&CL_I2C_SEQ)),
                    BusPhase::Free { .. } => Ok(()),
                }
            }
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &I2C_CLAUSES
    }
}

// I2C-READ-NACK-LAST and I2C-BYTE-TIME are checked by the harness that feeds
// this contract (it has direct access to per-byte durations and to whether a
// given DataByte was the last of a Read op), analogous to how the RTU
// contract's UART-level pairing is owned by the line contract. A thin helper
// is exposed for harnesses that observe raw (byte, is_last_of_read, dur):
impl I2cContract {
    /// Judge the READ-NACK-LAST obligation for one read-direction data byte.
    pub fn check_read_ack(&self, is_last: bool, acked: bool) -> Result<(), Breach> {
        if is_last && acked {
            return self.fail_master(Breach::of(&CL_I2C_READ_NACK_LAST));
        }
        if !is_last && !acked {
            return self.fail_master(Breach::of(&CL_I2C_READ_NACK_LAST));
        }
        Ok(())
    }

    /// Judge the BYTE-TIME obligation given an observed byte duration and
    /// the stretch already attributed to that byte.
    pub fn check_byte_duration(&self, dur: Nanos, stretch: Nanos) -> Result<(), Breach> {
        self.check_byte_time(dur, stretch)
    }
}
