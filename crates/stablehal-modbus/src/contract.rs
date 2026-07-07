//! The MODBUS RTU wire contract: every timing and framing rule of SL §2.5.1
//! plus the transaction discipline of SL §2.4.1, expressed as an executable
//! state machine over character events observed on the bus.
//!
//! The same contract instance judges simulated wires and hardware captures.
//! Each direction can be excluded from judgement (`judge_*` flags) so a
//! conformance tester may *deliberately* emit malformed stimuli while the
//! device under test is still held to every clause.

use core::mem;

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
    CharStart { dir: Dir, byte: u8 },
    CharEnd { dir: Dir, byte: u8 },
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
    text: "A response must start within [t3.5, response_timeout] after the request's last character.",
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
    /// after `gap` of silence, then open the frame.
    fn start_frame(
        &self,
        state: &mut RtuState,
        at: Nanos,
        gap: Nanos,
        dir: Dir,
        byte: u8,
    ) -> Result<(), Breach> {
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
        match *event {
            RtuEvent::CharStart { dir, byte } => {
                let phase = mem::replace(&mut state.line, LinePhase::Silent { since: at });
                match phase {
                    LinePhase::Silent { since } => {
                        let gap = at.saturating_sub(since);
                        self.start_frame(state, at, gap, dir, byte)
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
                            return self.start_frame(state, at, Nanos::ZERO, dir, byte);
                        }
                        let gap = at.saturating_sub(last_char_end);
                        if gap >= self.timing.t35 {
                            // Silence closed the previous frame; judge it,
                            // then open the new one.
                            self.finalize_frame(state, fdir, &frame, last_char_end, poisoned)?;
                            self.start_frame(state, at, gap, dir, byte)
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
