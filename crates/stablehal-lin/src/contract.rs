//! The LIN wire contract: frame sequencing, timing, and integrity, as an
//! executable state machine over frame-level bus events (LIN Specification
//! Package Rev 2.2A). Modeled at frame-field granularity (break / sync / PID
//! / response bytes / checksum), not bit-level UART framing — the same
//! split as the MODBUS RTU contract sitting above the UART line contract.
//!
//! The same contract instance judges simulated buses and hardware captures.
//! Each party can be excluded from judgement (`judge_master`/`judge_slaves`)
//! so a conformance tester may deliberately emit malformed stimuli while the
//! device under test is still held to every clause.

use core::mem;

use stablehal_contract::measure::Measurement;
use stablehal_contract::{Breach, Clause, Contract, Nanos, TimeBound};

use crate::timing::LinTiming;

/// Frame-level wire events, as observed by an independent bus monitor or
/// self-reported by a participant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinEvent {
    /// Master drives the break field low for `dur`.
    Break { dur: Nanos },
    /// Master drives the break delimiter (recessive) for `dur`.
    BreakDelimiter { dur: Nanos },
    /// Sync byte on the wire; must be exactly 0x55.
    Sync { byte: u8 },
    /// Protected identifier byte: `id` is the 6-bit frame ID (0..=63),
    /// `parity_ok` is whether the two parity bits match the LIN formula.
    Pid { id: u8, parity_ok: bool },
    /// One response data byte, `dur` since the previous byte on the wire
    /// (sync-to-PID gap excluded; measured PID-end-to-first-byte and
    /// byte-to-byte thereafter).
    DataByte { byte: u8, gap: Nanos },
    /// Checksum byte closing the response.
    Checksum { byte: u8, expected: u8, gap: Nanos },
    /// No response was driven at all for this frame's slot (an unassigned
    /// ID going unanswered, or a deliberately silent slave) — legal only
    /// when no configured responder claims this ID.
    NoResponse,
    /// Appended by the harness after all activity: finalizes an open frame
    /// so it does not escape judgement.
    EndOfObservation,
}

// --- Clauses (citation: LIN Specification Package Rev 2.2A) --------------

pub static CL_LIN_BREAK: Clause = Clause {
    id: "LIN-BREAK",
    citation: "LIN 2.2A §2.3.1.3",
    text: "The break field is driven low for at least 13 nominal bit times, \
           by the master, at the start of every frame.",
};
pub static CL_LIN_BREAK_DELIM: Clause = Clause {
    id: "LIN-BREAK-DELIM",
    citation: "LIN 2.2A §2.3.1.3",
    text: "The break delimiter (recessive) lasts at least 1 nominal bit time \
           between the break field and the sync byte.",
};
pub static CL_LIN_SYNC: Clause = Clause {
    id: "LIN-SYNC",
    citation: "LIN 2.2A §2.3.1.4",
    text: "The sync byte is exactly 0x55, giving receivers a bit-rate \
           reference edge pattern.",
};
pub static CL_LIN_PID_PARITY: Clause = Clause {
    id: "LIN-PID-PARITY",
    citation: "LIN 2.2A §2.3.1.5",
    text: "The protected identifier's two parity bits are computed correctly \
           from the 6-bit frame ID per the LIN parity formula.",
};
pub static CL_LIN_FRAME_MAXLEN: Clause = Clause {
    id: "LIN-FRAME-MAXLEN",
    citation: "LIN 2.2A §2.3.1.3 (T_frame_max = 1.4 x T_frame_nominal)",
    text: "The complete frame (header + response) fits within T_frame_max.",
};
pub static CL_LIN_INTERBYTE_MAX: Clause = Clause {
    id: "LIN-INTERBYTE-MAX",
    citation: "LIN 2.2A §2.3.1.3",
    text: "No inter-byte gap within the response exceeds the per-byte \
           tolerance budget (1.4x one nominal byte time).",
};
pub static CL_LIN_CHECKSUM: Clause = Clause {
    id: "LIN-CHECKSUM",
    citation: "LIN 2.2A §2.3.1.6",
    text: "The response checksum matches the data (classic: data bytes only; \
           enhanced: PID + data bytes), inverted mod-256 sum.",
};
pub static CL_LIN_SLAVE_SILENCE: Clause = Clause {
    id: "LIN-SLAVE-SILENCE",
    citation: "LIN 2.2A §2.3.1.2 (frame slot ownership)",
    text: "A slave not assigned to publish this frame ID never drives the \
           response; only the assigned publisher (or nobody, if unassigned) \
           responds.",
};
pub static CL_LIN_ONE_RESPONSE: Clause = Clause {
    id: "LIN-ONE-RESPONSE",
    citation: "LIN 2.2A §2.3.1.2",
    text: "At most one response is observed per frame slot (no bus \
           contention between multiple publishers of the same ID).",
};

static LIN_CLAUSES: [&Clause; 9] = [
    &CL_LIN_BREAK,
    &CL_LIN_BREAK_DELIM,
    &CL_LIN_SYNC,
    &CL_LIN_PID_PARITY,
    &CL_LIN_FRAME_MAXLEN,
    &CL_LIN_INTERBYTE_MAX,
    &CL_LIN_CHECKSUM,
    &CL_LIN_SLAVE_SILENCE,
    &CL_LIN_ONE_RESPONSE,
];

/// Maximum number of frame IDs with a registered publisher — fixed bound
/// keeps the contract `no_std`-friendly (ADR-9 style tradeoff), mirroring
/// `stablehal-i2c`'s `MAX_CONFIGURED_ADDRS`.
pub const MAX_PUBLISHERS: usize = 32;

#[derive(Copy, Clone, Debug)]
pub struct LinContract {
    pub timing: LinTiming,
    /// Judge the master's obligations (break, delimiter, sync, PID parity
    /// it generates, overall frame timing). Disabled when a conformance
    /// tester deliberately emits malformed stimuli.
    pub judge_master: bool,
    /// Judge slave/publisher obligations (the DUT in conformance runs).
    pub judge_slaves: bool,
    /// (frame_id, n_data) for every ID with a registered publisher — needed
    /// to size the frame budget and to judge LIN-SLAVE-SILENCE.
    publishers: [Option<(u8, u8)>; MAX_PUBLISHERS],
}

impl LinContract {
    pub fn new(timing: LinTiming, publishers: &[(u8, u8)]) -> Self {
        let mut regs = [None; MAX_PUBLISHERS];
        for (slot, &p) in regs.iter_mut().zip(publishers.iter()) {
            *slot = Some(p);
        }
        LinContract {
            timing,
            judge_master: true,
            judge_slaves: true,
            publishers: regs,
        }
    }

    fn publisher_n_data(&self, id: u8) -> Option<u8> {
        self.publishers
            .iter()
            .flatten()
            .find(|&&(pid, _)| pid == id)
            .map(|&(_, n)| n)
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

/// Standard LIN checksum: classic mode sums only the data bytes; enhanced
/// mode (used by default in LIN 2.x for non-diagnostic frames) also
/// includes the PID byte. This crate's reference implementation always
/// uses enhanced mode, matching LIN 2.2A's default for the frames it
/// defines; parameterizing classic-vs-enhanced per frame is left to a
/// future revision if a target deployment needs classic mode.
pub fn enhanced_checksum(pid: u8, data: &[u8]) -> u8 {
    let mut sum: u16 = pid as u16;
    for &b in data {
        sum += b as u16;
        if sum > 0xFF {
            sum -= 0xFF;
        }
    }
    !(sum as u8)
}

/// LIN protected identifier parity, per LIN 2.2A §2.3.1.5: P0 = ID0^ID1^ID2^ID4,
/// P1 = !(ID1^ID3^ID4^ID5).
pub fn pid_parity_ok(id: u8, parity_bits: u8) -> bool {
    encode_pid(id) >> 6 == parity_bits
}

/// Builds the full protected identifier byte (parity bits in the top two
/// bits) from a 6-bit frame ID.
pub const fn encode_pid(id: u8) -> u8 {
    let id = id & 0x3F;
    let id0 = id & 1;
    let id1 = (id >> 1) & 1;
    let id2 = (id >> 2) & 1;
    let id3 = (id >> 3) & 1;
    let id4 = (id >> 4) & 1;
    let id5 = (id >> 5) & 1;
    let p0 = id0 ^ id1 ^ id2 ^ id4;
    let p1 = !(id1 ^ id3 ^ id4 ^ id5) & 1;
    id | (p0 << 6) | (p1 << 7)
}

#[derive(Clone, Debug)]
enum FramePhase {
    /// Idle between frames; no break seen yet.
    Idle,
    /// Break observed at `break_at`, awaiting delimiter.
    BreakSeen { break_at: Nanos },
    /// Delimiter observed; awaiting sync byte.
    DelimiterSeen { break_at: Nanos },
    /// Sync byte observed (and must be 0x55, checked at the time); awaiting
    /// PID.
    SyncSeen { break_at: Nanos },
    /// PID observed; header complete, now in the response window.
    /// `bytes_seen` counts data bytes only (checksum ends the frame).
    Header {
        break_at: Nanos,
        id: u8,
        bytes_seen: u8,
        last_event_at: Nanos,
        responded: bool,
    },
}

#[derive(Clone, Debug)]
pub struct LinState {
    phase: FramePhase,
}

impl Contract for LinContract {
    type State = LinState;
    type Event = LinEvent;

    fn initial(&self) -> LinState {
        LinState {
            phase: FramePhase::Idle,
        }
    }

    fn step(&self, state: &mut LinState, at: Nanos, event: &LinEvent) -> Result<(), Breach> {
        self.step_measured(state, at, event, &mut |_| {})
    }

    fn step_measured(
        &self,
        state: &mut LinState,
        at: Nanos,
        event: &LinEvent,
        sink: &mut dyn FnMut(Measurement),
    ) -> Result<(), Breach> {
        match *event {
            LinEvent::Break { dur } => {
                sink(Measurement {
                    clause_id: CL_LIN_BREAK.id,
                    value: dur,
                });
                if dur < self.timing.break_min() {
                    self.fail_master(Breach::timing(
                        &CL_LIN_BREAK,
                        dur,
                        TimeBound::at_least(self.timing.break_min()),
                    ))?;
                }
                state.phase = FramePhase::BreakSeen { break_at: at };
                Ok(())
            }
            LinEvent::BreakDelimiter { dur } => {
                let break_at = match state.phase {
                    FramePhase::BreakSeen { break_at } => break_at,
                    _ => return self.fail_master(Breach::of(&CL_LIN_BREAK)),
                };
                sink(Measurement {
                    clause_id: CL_LIN_BREAK_DELIM.id,
                    value: dur,
                });
                if dur < self.timing.break_delimiter_min() {
                    self.fail_master(Breach::timing(
                        &CL_LIN_BREAK_DELIM,
                        dur,
                        TimeBound::at_least(self.timing.break_delimiter_min()),
                    ))?;
                }
                state.phase = FramePhase::DelimiterSeen { break_at };
                Ok(())
            }
            LinEvent::Sync { byte } => {
                let break_at = match state.phase {
                    FramePhase::DelimiterSeen { break_at } => break_at,
                    _ => return self.fail_master(Breach::of(&CL_LIN_SYNC)),
                };
                if byte != 0x55 {
                    self.fail_master(Breach::of(&CL_LIN_SYNC))?;
                }
                state.phase = FramePhase::SyncSeen { break_at };
                Ok(())
            }
            LinEvent::Pid { id, parity_ok } => {
                let break_at = match state.phase {
                    FramePhase::SyncSeen { break_at } => break_at,
                    _ => return self.fail_master(Breach::of(&CL_LIN_PID_PARITY)),
                };
                if !parity_ok {
                    self.fail_master(Breach::of(&CL_LIN_PID_PARITY))?;
                }
                state.phase = FramePhase::Header {
                    break_at,
                    id,
                    bytes_seen: 0,
                    last_event_at: at,
                    responded: false,
                };
                Ok(())
            }
            LinEvent::DataByte { gap, .. } => {
                if let FramePhase::Header {
                    bytes_seen,
                    last_event_at,
                    responded,
                    ..
                } = &mut state.phase
                {
                    sink(Measurement {
                        clause_id: CL_LIN_INTERBYTE_MAX.id,
                        value: gap,
                    });
                    if gap > self.timing.interbyte_max() {
                        self.fail_slave(Breach::timing(
                            &CL_LIN_INTERBYTE_MAX,
                            gap,
                            TimeBound::new(Nanos::ZERO, self.timing.interbyte_max()),
                        ))?;
                    }
                    *bytes_seen += 1;
                    *last_event_at = at;
                    *responded = true;
                    Ok(())
                } else {
                    self.fail_slave(Breach::of(&CL_LIN_ONE_RESPONSE))
                }
            }
            LinEvent::Checksum {
                byte,
                expected,
                gap,
            } => {
                let (break_at, id, n_data) = match &mut state.phase {
                    FramePhase::Header {
                        break_at,
                        id,
                        bytes_seen,
                        responded,
                        ..
                    } => {
                        *responded = true;
                        (*break_at, *id, *bytes_seen)
                    }
                    _ => return self.fail_slave(Breach::of(&CL_LIN_ONE_RESPONSE)),
                };
                sink(Measurement {
                    clause_id: CL_LIN_INTERBYTE_MAX.id,
                    value: gap,
                });
                if gap > self.timing.interbyte_max() {
                    self.fail_slave(Breach::timing(
                        &CL_LIN_INTERBYTE_MAX,
                        gap,
                        TimeBound::new(Nanos::ZERO, self.timing.interbyte_max()),
                    ))?;
                }
                if byte != expected {
                    self.fail_slave(Breach::of(&CL_LIN_CHECKSUM))?;
                }
                let elapsed = at.saturating_sub(break_at);
                let max = self.timing.frame_max(n_data);
                sink(Measurement {
                    clause_id: CL_LIN_FRAME_MAXLEN.id,
                    value: elapsed,
                });
                if elapsed > max {
                    self.fail_master(Breach::timing(
                        &CL_LIN_FRAME_MAXLEN,
                        elapsed,
                        TimeBound::new(Nanos::ZERO, max),
                    ))?;
                }
                let _ = id;
                state.phase = FramePhase::Idle;
                Ok(())
            }
            LinEvent::NoResponse => {
                let id = match state.phase {
                    FramePhase::Header { id, .. } => id,
                    _ => return self.fail_master(Breach::of(&CL_LIN_PID_PARITY)),
                };
                if self.publisher_n_data(id).is_some() {
                    self.fail_slave(Breach::of(&CL_LIN_SLAVE_SILENCE))?;
                }
                state.phase = FramePhase::Idle;
                Ok(())
            }
            LinEvent::EndOfObservation => {
                let phase = mem::replace(&mut state.phase, FramePhase::Idle);
                match phase {
                    FramePhase::Idle => Ok(()),
                    _ => self.fail_master(Breach::of(&CL_LIN_FRAME_MAXLEN)),
                }
            }
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &LIN_CLAUSES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::oracle::check_trace;
    use stablehal_contract::{TraceEvent, Verdict};

    fn timing() -> LinTiming {
        LinTiming::new(19_200)
    }

    #[test]
    fn pid_parity_matches_reference_table() {
        // LIN 2.2A worked examples: ID 0x00 -> PID 0x80, ID 0x01 -> PID 0xC1.
        assert_eq!(encode_pid(0x00), 0x80);
        assert_eq!(encode_pid(0x01), 0xC1);
    }

    #[test]
    fn conforming_frame_passes() {
        let t = timing();
        let contract = LinContract::new(t, &[(0x10, 2)]);
        let pid = encode_pid(0x10);
        let data = [0xAAu8, 0xBB];
        let checksum = enhanced_checksum(pid, &data);
        let trace = [
            TraceEvent {
                at: Nanos::ZERO,
                event: LinEvent::Break { dur: t.break_min() },
            },
            TraceEvent {
                at: t.break_min(),
                event: LinEvent::BreakDelimiter {
                    dur: t.break_delimiter_min(),
                },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min(),
                event: LinEvent::Sync { byte: 0x55 },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min() + t.byte_time(),
                event: LinEvent::Pid {
                    id: 0x10,
                    parity_ok: true,
                },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min() + t.byte_time() + t.byte_time(),
                event: LinEvent::DataByte {
                    byte: 0xAA,
                    gap: t.byte_time(),
                },
            },
            TraceEvent {
                at: t.break_min()
                    + t.break_delimiter_min()
                    + t.byte_time()
                    + t.byte_time()
                    + t.byte_time(),
                event: LinEvent::DataByte {
                    byte: 0xBB,
                    gap: t.byte_time(),
                },
            },
            TraceEvent {
                at: t.break_min()
                    + t.break_delimiter_min()
                    + t.byte_time()
                    + t.byte_time()
                    + t.byte_time()
                    + t.byte_time(),
                event: LinEvent::Checksum {
                    byte: checksum,
                    expected: checksum,
                    gap: t.byte_time(),
                },
            },
            TraceEvent {
                at: t.frame_max(2),
                event: LinEvent::EndOfObservation,
            },
        ];
        let verdict = check_trace(contract, &trace);
        assert!(matches!(verdict, Verdict::Conforming { .. }), "{verdict:?}");
    }

    #[test]
    fn short_break_is_caught() {
        let t = timing();
        let contract = LinContract::new(t, &[]);
        let trace = [TraceEvent {
            at: Nanos::ZERO,
            event: LinEvent::Break {
                dur: t.break_min().saturating_sub(Nanos::from_nanos(1)),
            },
        }];
        let verdict = check_trace(contract, &trace);
        match verdict {
            Verdict::Violated(v) => assert_eq!(v.breach.clause.id, "LIN-BREAK"),
            other => panic!("expected LIN-BREAK violation, got {other:?}"),
        }
    }

    #[test]
    fn bad_checksum_is_caught() {
        let t = timing();
        let contract = LinContract::new(t, &[(0x01, 1)]);
        let pid = encode_pid(0x01);
        let data = [0x42u8];
        let checksum = enhanced_checksum(pid, &data);
        let trace = [
            TraceEvent {
                at: Nanos::ZERO,
                event: LinEvent::Break { dur: t.break_min() },
            },
            TraceEvent {
                at: t.break_min(),
                event: LinEvent::BreakDelimiter {
                    dur: t.break_delimiter_min(),
                },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min(),
                event: LinEvent::Sync { byte: 0x55 },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min() + t.byte_time(),
                event: LinEvent::Pid {
                    id: 0x01,
                    parity_ok: true,
                },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min() + t.byte_time() + t.byte_time(),
                event: LinEvent::DataByte {
                    byte: 0x42,
                    gap: t.byte_time(),
                },
            },
            TraceEvent {
                at: t.break_min()
                    + t.break_delimiter_min()
                    + t.byte_time()
                    + t.byte_time()
                    + t.byte_time(),
                event: LinEvent::Checksum {
                    byte: checksum.wrapping_add(1),
                    expected: checksum,
                    gap: t.byte_time(),
                },
            },
        ];
        let verdict = check_trace(contract, &trace);
        match verdict {
            Verdict::Violated(v) => assert_eq!(v.breach.clause.id, "LIN-CHECKSUM"),
            other => panic!("expected LIN-CHECKSUM violation, got {other:?}"),
        }
    }

    #[test]
    fn silent_configured_publisher_is_caught() {
        let t = timing();
        let contract = LinContract::new(t, &[(0x05, 1)]);
        let trace = [
            TraceEvent {
                at: Nanos::ZERO,
                event: LinEvent::Break { dur: t.break_min() },
            },
            TraceEvent {
                at: t.break_min(),
                event: LinEvent::BreakDelimiter {
                    dur: t.break_delimiter_min(),
                },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min(),
                event: LinEvent::Sync { byte: 0x55 },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min() + t.byte_time(),
                event: LinEvent::Pid {
                    id: 0x05,
                    parity_ok: true,
                },
            },
            TraceEvent {
                at: t.break_min() + t.break_delimiter_min() + t.byte_time() + t.byte_time(),
                event: LinEvent::NoResponse,
            },
        ];
        let verdict = check_trace(contract, &trace);
        match verdict {
            Verdict::Violated(v) => assert_eq!(v.breach.clause.id, "LIN-SLAVE-SILENCE"),
            other => panic!("expected LIN-SLAVE-SILENCE violation, got {other:?}"),
        }
    }
}
