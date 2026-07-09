//! The MODBUS RTU wire contract: every timing and framing rule of SL §2.5.1
//! plus the transaction discipline of SL §2.4.1, expressed as an executable
//! state machine over character events observed on the bus.
//!
//! The same contract instance judges simulated wires and hardware captures.
//! Each direction can be excluded from judgement (`judge_*` flags) so a
//! conformance tester may *deliberately* emit malformed stimuli while the
//! device under test is still held to every clause.

#![allow(clippy::large_enum_variant)]
// ADR-9: Adu is a fixed 256-byte buffer; the size difference across
// LinePhase variants is accepted knowingly.

use core::mem;

use stablehal_contract::measure::Measurement;
use stablehal_contract::{Breach, Clause, Contract, Nanos, TimeBound};

use crate::crc;
use crate::pdu::{self, Adu};
use crate::timing::RtuTiming;

/// Wire direction. `MasterToSlave` is by convention line 0.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Dir {
    MasterToSlave,
    SlaveToMaster,
}

/// Character-level events, derived from the UART line events of both
/// directions (transmitter side only — what each party actually emitted).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RtuEvent {
    CharStart {
        dir: Dir,
        byte: u8,
    },
    CharEnd {
        dir: Dir,
        byte: u8,
    },
    /// Appended by the harness after all activity (+ ≥ t3.5): finalizes a
    /// trailing frame so it does not escape judgement.
    EndOfObservation,
}

// --- Clauses (traceability: SL = MODBUS over Serial Line V1.02, AP =
// MODBUS Application Protocol V1.1b3) -----------------------------------

pub static CL_RTU_T35_FRAME: Clause = Clause {
    id: "RTU-T35-FRAME",
    citation: "SL §2.5.1.1",
    text: "Frames must be separated by at least t3.5 of silence.",
};
pub static CL_RTU_T15_INTRA: Clause = Clause {
    id: "RTU-T15-INTRA",
    citation: "SL §2.5.1.1",
    text: "Silence between characters of one frame must not exceed t1.5.",
};
pub static CL_RTU_CHAR_OVERLAP: Clause = Clause {
    id: "RTU-CHAR-OVERLAP",
    citation: "SL §2.5.1 (half-duplex discipline)",
    text: "A party must not start a character while another character is on the bus.",
};
pub static CL_RTU_FRAME_MIN: Clause = Clause {
    id: "RTU-FRAME-MIN",
    citation: "SL §2.5.1",
    text: "A frame is at least 4 bytes: address, function code, CRC-16.",
};
pub static CL_RTU_CRC: Clause = Clause {
    id: "RTU-CRC",
    citation: "SL §2.5.1.2",
    text: "Every emitted frame must carry a valid CRC-16 (low byte first).",
};
pub static CL_RTU_PDU_FORM: Clause = Clause {
    id: "RTU-PDU-FORM",
    citation: "AP §6",
    text: "PDU length and fields must be consistent with the function code.",
};
pub static CL_RTU_RESP_UNIT: Clause = Clause {
    id: "RTU-RESP-UNIT",
    citation: "SL §2.4.1",
    text: "A response must carry the unit address of the request it answers.",
};
pub static CL_RTU_RESP_FC: Clause = Clause {
    id: "RTU-RESP-FC",
    citation: "AP §7",
    text: "A response echoes the request function code, or fc|0x80 for an exception.",
};
pub static CL_RTU_EXC_CODE: Clause = Clause {
    id: "RTU-EXC-CODE",
    citation: "AP §7",
    text: "An exception response is exactly unit, fc|0x80, valid code, CRC.",
};
pub static CL_RTU_RESP_TIME: Clause = Clause {
    id: "RTU-RESP-TIME",
    citation: "SL §2.4.1 + §2.5.1.1",
    text:
        "A response must start within [t3.5, response_timeout] after the request's last character.",
};
pub static CL_RTU_MASTER_WAIT: Clause = Clause {
    id: "RTU-MASTER-WAIT",
    citation: "SL §2.4.1",
    text: "The master must not issue a new request before the response completes or the response timeout expires.",
};
pub static CL_RTU_UNSOLICITED: Clause = Clause {
    id: "RTU-UNSOLICITED",
    citation: "SL §2.4.2",
    text: "A slave transmits only in response to a request.",
};

static RTU_CLAUSES: [&Clause; 12] = [
    &CL_RTU_T35_FRAME,
    &CL_RTU_T15_INTRA,
    &CL_RTU_CHAR_OVERLAP,
    &CL_RTU_FRAME_MIN,
    &CL_RTU_CRC,
    &CL_RTU_PDU_FORM,
    &CL_RTU_RESP_UNIT,
    &CL_RTU_RESP_FC,
    &CL_RTU_EXC_CODE,
    &CL_RTU_RESP_TIME,
    &CL_RTU_MASTER_WAIT,
    &CL_RTU_UNSOLICITED,
];

// --- Contract ----------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct RtuContract {
    pub timing: RtuTiming,
    /// Judge the master→slave direction. Disabled when a conformance tester
    /// deliberately emits malformed stimuli.
    pub judge_master: bool,
    /// Judge the slave→master direction (the DUT in conformance runs).
    pub judge_slave: bool,
}

impl RtuContract {
    pub fn new(timing: RtuTiming) -> Self {
        RtuContract {
            timing,
            judge_master: true,
            judge_slave: true,
        }
    }

    fn judged(&self, dir: Dir) -> bool {
        match dir {
            Dir::MasterToSlave => self.judge_master,
            Dir::SlaveToMaster => self.judge_slave,
        }
    }

    /// Raise `breach` only if `dir` is under judgement.
    fn fail(&self, dir: Dir, breach: Breach) -> Result<(), Breach> {
        if self.judged(dir) {
            Err(breach)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
enum LinePhase {
    Silent {
        since: Nanos,
    },
    InFrame {
        dir: Dir,
        frame: Adu,
        last_char_end: Nanos,
        in_char: Option<u8>,
        /// Set when an unjudged party broke framing mid-frame: the bytes are
        /// then line noise, carrying no protocol meaning.
        poisoned: bool,
    },
}

#[derive(Clone, Debug)]
enum Proto {
    ExpectRequest,
    AwaitResponse {
        unit: u8,
        fc: u8,
        request_end: Nanos,
    },
}

#[derive(Clone, Debug)]
pub struct RtuState {
    line: LinePhase,
    proto: Proto,
}

impl RtuContract {
    /// Gap/protocol checks when a new frame's first character starts at `at`
    /// after `gap` of silence, then open the frame. Every timing duration
    /// this computes is reported to `sink` before the bound check, so a
    /// passing (or waived) measurement still contributes to the envelope
    /// hull — not just the ones that breach.
    fn start_frame(
        &self,
        state: &mut RtuState,
        at: Nanos,
        gap: Nanos,
        dir: Dir,
        byte: u8,
        sink: &mut dyn FnMut(Measurement),
    ) -> Result<(), Breach> {
        sink(Measurement {
            clause_id: CL_RTU_T35_FRAME.id,
            value: gap,
        });
        if gap < self.timing.t35 {
            self.fail(
                dir,
                Breach::timing(&CL_RTU_T35_FRAME, gap, TimeBound::at_least(self.timing.t35)),
            )?;
        }
        match (dir, &state.proto) {
            (Dir::MasterToSlave, Proto::ExpectRequest) => {}
            (Dir::MasterToSlave, Proto::AwaitResponse { request_end, .. }) => {
                let waited = at.saturating_sub(*request_end);
                sink(Measurement {
                    clause_id: CL_RTU_MASTER_WAIT.id,
                    value: waited,
                });
                if waited < self.timing.response_timeout {
                    self.fail(
                        Dir::MasterToSlave,
                        Breach::timing(
                            &CL_RTU_MASTER_WAIT,
                            waited,
                            TimeBound::at_least(self.timing.response_timeout),
                        ),
                    )?;
                }
                // Legal retry (or waived breach): the pending expectation is
                // abandoned; the new frame will set a fresh one.
                state.proto = Proto::ExpectRequest;
            }
            (Dir::SlaveToMaster, Proto::ExpectRequest) => {
                self.fail(Dir::SlaveToMaster, Breach::of(&CL_RTU_UNSOLICITED))?;
            }
            (Dir::SlaveToMaster, Proto::AwaitResponse { request_end, .. }) => {
                let delay = at.saturating_sub(*request_end);
                sink(Measurement {
                    clause_id: CL_RTU_RESP_TIME.id,
                    value: delay,
                });
                let envelope = self.timing.response_envelope();
                if !envelope.contains(delay) {
                    self.fail(
                        Dir::SlaveToMaster,
                        Breach::timing(&CL_RTU_RESP_TIME, delay, envelope),
                    )?;
                }
            }
        }
        state.line = LinePhase::InFrame {
            dir,
            frame: Adu::new(),
            last_char_end: at,
            in_char: Some(byte),
            poisoned: false,
        };
        Ok(())
    }

    /// Close a frame and apply protocol-level judgement.
    fn finalize_frame(
        &self,
        state: &mut RtuState,
        dir: Dir,
        frame: &Adu,
        frame_end: Nanos,
        poisoned: bool,
    ) -> Result<(), Breach> {
        state.line = LinePhase::Silent { since: frame_end };
        if poisoned {
            return Ok(()); // line noise from an unjudged party
        }
        let bytes = frame.bytes();
        if bytes.len() < 4 {
            self.fail(dir, Breach::of(&CL_RTU_FRAME_MIN))?;
            return Ok(()); // waived: noise, no protocol meaning
        }
        if !crc::check(bytes) {
            self.fail(dir, Breach::of(&CL_RTU_CRC))?;
            return Ok(()); // waived: receivers discard it silently
        }
        match dir {
            Dir::MasterToSlave => match pdu::parse_request(bytes) {
                Ok((unit, _)) => {
                    state.proto = Proto::AwaitResponse {
                        unit,
                        fc: bytes[1],
                        request_end: frame_end,
                    };
                }
                Err(_) => {
                    self.fail(dir, Breach::of(&CL_RTU_PDU_FORM))?;
                    // Waived: malformed request, slaves discard; no expectation.
                }
            },
            Dir::SlaveToMaster => {
                if let Proto::AwaitResponse { unit, fc, .. } = state.proto {
                    if bytes[0] != unit {
                        self.fail(dir, Breach::of(&CL_RTU_RESP_UNIT))?;
                    } else {
                        let rfc = bytes[1];
                        if rfc == fc | 0x80 {
                            let valid_exc =
                                bytes.len() == 5 && pdu::ExceptionCode::from_u8(bytes[2]).is_some();
                            if !valid_exc {
                                self.fail(dir, Breach::of(&CL_RTU_EXC_CODE))?;
                            }
                        } else if rfc == fc {
                            if pdu::parse_response(unit, fc, bytes).is_err() {
                                self.fail(dir, Breach::of(&CL_RTU_PDU_FORM))?;
                            }
                        } else {
                            self.fail(dir, Breach::of(&CL_RTU_RESP_FC))?;
                        }
                    }
                    state.proto = Proto::ExpectRequest;
                }
                // Unsolicited frames were already flagged at frame start.
            }
        }
        Ok(())
    }
}

impl Contract for RtuContract {
    type State = RtuState;
    type Event = RtuEvent;

    fn initial(&self) -> RtuState {
        RtuState {
            line: LinePhase::Silent { since: Nanos::ZERO },
            proto: Proto::ExpectRequest,
        }
    }

    fn step(&self, state: &mut RtuState, at: Nanos, event: &RtuEvent) -> Result<(), Breach> {
        self.step_measured(state, at, event, &mut |_| {})
    }

    fn step_measured(
        &self,
        state: &mut RtuState,
        at: Nanos,
        event: &RtuEvent,
        sink: &mut dyn FnMut(Measurement),
    ) -> Result<(), Breach> {
        match *event {
            RtuEvent::CharStart { dir, byte } => {
                let phase = mem::replace(&mut state.line, LinePhase::Silent { since: at });
                match phase {
                    LinePhase::Silent { since } => {
                        let gap = at.saturating_sub(since);
                        self.start_frame(state, at, gap, dir, byte, sink)
                    }
                    LinePhase::InFrame {
                        dir: fdir,
                        frame,
                        last_char_end,
                        in_char,
                        poisoned,
                    } => {
                        if in_char.is_some() {
                            // Character-level collision: someone keyed up
                            // while a character was mid-flight.
                            self.fail(dir, Breach::of(&CL_RTU_CHAR_OVERLAP))?;
                            // Waived: drop the frame as noise and start over.
                            state.line = LinePhase::Silent { since: at };
                            return self.start_frame(state, at, Nanos::ZERO, dir, byte, sink);
                        }
                        let gap = at.saturating_sub(last_char_end);
                        if gap >= self.timing.t35 {
                            // Silence closed the previous frame; judge it,
                            // then open the new one.
                            self.finalize_frame(state, fdir, &frame, last_char_end, poisoned)?;
                            self.start_frame(state, at, gap, dir, byte, sink)
                        } else if dir != fdir {
                            // Direction change without frame separation: the
                            // newcomer barged in.
                            self.fail(
                                dir,
                                Breach::timing(
                                    &CL_RTU_T35_FRAME,
                                    gap,
                                    TimeBound::at_least(self.timing.t35),
                                ),
                            )?;
                            // Waived: old frame dies as noise, new frame opens.
                            self.finalize_frame(state, fdir, &frame, last_char_end, true)?;
                            state.line = LinePhase::InFrame {
                                dir,
                                frame: Adu::new(),
                                last_char_end: at,
                                in_char: Some(byte),
                                poisoned: false,
                            };
                            Ok(())
                        } else {
                            let mut poisoned = poisoned;
                            sink(Measurement {
                                clause_id: CL_RTU_T15_INTRA.id,
                                value: gap,
                            });
                            if gap > self.timing.t15 {
                                self.fail(
                                    dir,
                                    Breach::timing(
                                        &CL_RTU_T15_INTRA,
                                        gap,
                                        TimeBound::new(Nanos::ZERO, self.timing.t15),
                                    ),
                                )?;
                                // Waived: receivers will discard this frame.
                                poisoned = true;
                            }
                            state.line = LinePhase::InFrame {
                                dir: fdir,
                                frame,
                                last_char_end,
                                in_char: Some(byte),
                                poisoned,
                            };
                            Ok(())
                        }
                    }
                }
            }
            RtuEvent::CharEnd { dir, byte } => {
                if let LinePhase::InFrame {
                    dir: fdir,
                    frame,
                    last_char_end,
                    in_char,
                    ..
                } = &mut state.line
                {
                    if *fdir == dir && *in_char == Some(byte) {
                        frame.push(byte);
                        *last_char_end = at;
                        *in_char = None;
                    }
                    // Pairing mismatches are owned by the UART line contract.
                }
                Ok(())
            }
            RtuEvent::EndOfObservation => {
                let phase = mem::replace(&mut state.line, LinePhase::Silent { since: at });
                if let LinePhase::InFrame {
                    dir,
                    frame,
                    last_char_end,
                    poisoned,
                    ..
                } = phase
                {
                    self.finalize_frame(state, dir, &frame, last_char_end, poisoned)?;
                }
                Ok(())
            }
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &RTU_CLAUSES
    }
}

#[cfg(test)]
mod measure_tests {
    use super::*;
    use stablehal_contract::EnvelopeAccumulator;

    // 11:03 read-holding, addr 0, qty 1, CRC 0x9a86 (low byte first).
    const REQUEST: [u8; 8] = [0x11, 0x03, 0x00, 0x00, 0x00, 0x01, 0x86, 0x9a];
    // 11:03, byte count 2, data 0x0005, CRC 0x84b9 (low byte first).
    const RESPONSE: [u8; 7] = [0x11, 0x03, 0x02, 0x00, 0x05, 0xb9, 0x84];

    fn timing() -> RtuTiming {
        RtuTiming::from_uart(
            &stablehal_uart::UartConfig::rtu_default(9600),
            Nanos::from_millis(50),
        )
    }

    /// Feed one request/response transaction with legal, mid-envelope gaps,
    /// driving `step_measured` directly to inspect what it reports.
    fn drive_conforming_transaction(
        contract: &RtuContract,
        sink: &mut dyn FnMut(Measurement),
    ) -> RtuState {
        let mut state = contract.initial();
        let mut at = Nanos::from_millis(10);
        let step_char = |at: &mut Nanos,
                         dir: Dir,
                         byte: u8,
                         contract: &RtuContract,
                         state: &mut RtuState,
                         sink: &mut dyn FnMut(Measurement)| {
            contract
                .step_measured(state, *at, &RtuEvent::CharStart { dir, byte }, sink)
                .unwrap();
            *at += contract.timing.char_time;
            contract
                .step_measured(state, *at, &RtuEvent::CharEnd { dir, byte }, sink)
                .unwrap();
        };
        for &b in &REQUEST {
            step_char(&mut at, Dir::MasterToSlave, b, contract, &mut state, sink);
        }
        at += contract.timing.t35 + Nanos::from_micros(10);
        for &b in &RESPONSE {
            step_char(&mut at, Dir::SlaveToMaster, b, contract, &mut state, sink);
        }
        contract
            .step_measured(&mut state, at, &RtuEvent::EndOfObservation, sink)
            .unwrap();
        state
    }

    #[test]
    fn step_and_step_measured_agree_on_a_conforming_trace() {
        let contract = RtuContract::new(timing());
        let mut via_step = contract.initial();
        let mut via_measured = contract.initial();
        let mut at = Nanos::from_millis(10);

        for &b in &REQUEST {
            let ev_start = RtuEvent::CharStart {
                dir: Dir::MasterToSlave,
                byte: b,
            };
            contract.step(&mut via_step, at, &ev_start).unwrap();
            contract
                .step_measured(&mut via_measured, at, &ev_start, &mut |_| {})
                .unwrap();
            at += contract.timing.char_time;
            let ev_end = RtuEvent::CharEnd {
                dir: Dir::MasterToSlave,
                byte: b,
            };
            contract.step(&mut via_step, at, &ev_end).unwrap();
            contract
                .step_measured(&mut via_measured, at, &ev_end, &mut |_| {})
                .unwrap();
        }
        // no_std: compare via a Debug-driven hash rather than `format!`.
        use core::fmt::Write as _;
        let mut a = stablehal_contract::trace::HashWriter::new();
        let mut b = stablehal_contract::trace::HashWriter::new();
        write!(a, "{via_step:?}").unwrap();
        write!(b, "{via_measured:?}").unwrap();
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn measured_gaps_are_reported_even_when_conforming() {
        let contract = RtuContract::new(timing());
        let mut acc: EnvelopeAccumulator<8> = EnvelopeAccumulator::new();
        let mut sink = |m: Measurement| acc.observe(m);
        drive_conforming_transaction(&contract, &mut sink);

        // T3.5 was measured at least once: the gap before the response.
        let t35 = acc.hull_for(CL_RTU_T35_FRAME.id).expect("T3.5 measured");
        assert!(t35.samples >= 1);
        assert!(t35.min >= contract.timing.t35);

        // T1.5 (intra-frame gaps) measured many times — one per interior byte.
        let t15 = acc.hull_for(CL_RTU_T15_INTRA.id).expect("T1.5 measured");
        assert!(t15.samples > 0);
        assert!(t15.max <= contract.timing.t15);

        // Response start delay was measured and falls inside the envelope.
        let resp = acc
            .hull_for(CL_RTU_RESP_TIME.id)
            .expect("resp time measured");
        let envelope = contract.timing.response_envelope();
        assert!(envelope.contains(resp.min));
        assert!(envelope.contains(resp.max));
    }
}
