//! Contract-derived conformance suite for MODBUS RTU slaves.
//!
//! The case definitions are `no_std`: the identical suite can later execute
//! from an on-target runner against real hardware. The simulator-hosted
//! runner lives behind the `runner` feature.
//!
//! Precondition (part of the suite specification): before a case runs, the
//! DUT's holding registers are initialized to `pattern(addr)` for every
//! address. A hardware fixture must arrange the same.

#![cfg_attr(not(feature = "runner"), no_std)]
#![deny(unsafe_code)]

#[cfg(feature = "runner")]
pub mod runner;

/// Register initialization pattern the suite assumes.
pub fn pattern(addr: u16) -> u16 {
    0xA000 | addr
}

/// The unit address the DUT must be configured with.
pub const DUT_UNIT: u8 = 0x11;

/// Requests the tester can build.
#[derive(Copy, Clone, Debug)]
pub enum CaseRequest {
    ReadHolding { addr: u16, count: u16 },
    WriteSingle { addr: u16, value: u16 },
    WriteMultiple { addr: u16, values: &'static [u16] },
}

impl CaseRequest {
    pub fn function_code(&self) -> u8 {
        match self {
            CaseRequest::ReadHolding { .. } => 0x03,
            CaseRequest::WriteSingle { .. } => 0x06,
            CaseRequest::WriteMultiple { .. } => 0x10,
        }
    }
}

/// Deliberate inter-character gap sizes for timing probes.
#[derive(Copy, Clone, Debug)]
pub enum GapKind {
    /// Exactly t1.5 — the largest *legal* intra-frame silence; the DUT must
    /// still accept the frame.
    ExactlyT15,
    /// Comfortably beyond t3.5 (t3.5 + 250 µs, clear of receive jitter): the
    /// frame splits into two invalid pieces; the DUT must stay silent.
    WellOverT35,
}

/// One wire stimulus emitted by the tester.
#[derive(Copy, Clone, Debug)]
pub enum Stimulus {
    /// Well-formed request to [`DUT_UNIT`].
    Valid(CaseRequest),
    /// Well-formed request addressed to a different unit.
    WrongUnit(CaseRequest),
    /// Valid request with the final CRC byte flipped.
    CorruptCrc(CaseRequest),
    /// CRC-valid frame carrying an out-of-subset function code.
    UnknownFc { fc: u8 },
    /// A 3-byte fragment (below the minimum frame size).
    Runt,
    /// Valid request with a deliberate silence inserted after `after_byte`.
    MidFrameGap {
        req: CaseRequest,
        after_byte: usize,
        gap: GapKind,
    },
}

/// What the DUT must do in response.
#[derive(Copy, Clone, Debug)]
pub enum Expected {
    /// FC03 response carrying `pattern(addr+i)` for each register.
    ReadPattern {
        addr: u16,
        count: u16,
    },
    /// FC03 response carrying exactly these values.
    ReadValues {
        values: &'static [u16],
    },
    EchoWriteSingle {
        addr: u16,
        value: u16,
    },
    EchoWriteMultiple {
        addr: u16,
        count: u16,
    },
    Exception {
        code: u8,
    },
    /// No transmission at all within the response window.
    Silence,
}

#[derive(Copy, Clone, Debug)]
pub struct Step {
    pub stimulus: Stimulus,
    pub expect: Expected,
}

#[derive(Copy, Clone, Debug)]
pub struct Case {
    pub id: &'static str,
    pub description: &'static str,
    /// Contract clause ids this case exercises (coverage accounting).
    pub probes: &'static [&'static str],
    /// False when the tester deliberately violates master-side clauses; the
    /// wire oracle then judges only the DUT direction.
    pub judge_master_dir: bool,
    pub steps: &'static [Step],
}

pub static STANDARD_SUITE: &[Case] = &[
    Case {
        id: "READ-BASIC",
        description: "FC03 read of 10 registers returns the pattern",
        probes: &[
            "RTU-RESP-TIME",
            "RTU-T35-FRAME",
            "RTU-CRC",
            "RTU-RESP-UNIT",
            "RTU-RESP-FC",
        ],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::Valid(CaseRequest::ReadHolding { addr: 0, count: 10 }),
            expect: Expected::ReadPattern { addr: 0, count: 10 },
        }],
    },
    Case {
        id: "READ-MAX",
        description: "FC03 read of the maximum 125 registers",
        probes: &["RTU-PDU-FORM"],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::Valid(CaseRequest::ReadHolding {
                addr: 0,
                count: 125,
            }),
            expect: Expected::ReadPattern {
                addr: 0,
                count: 125,
            },
        }],
    },
    Case {
        id: "READ-BAD-ADDR",
        description: "FC03 past the register space yields exception 02",
        probes: &["RTU-EXC-CODE"],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::Valid(CaseRequest::ReadHolding {
                addr: 120,
                count: 20,
            }),
            expect: Expected::Exception { code: 0x02 },
        }],
    },
    Case {
        id: "READ-BAD-COUNT",
        description: "FC03 with quantity 0 yields exception 03",
        probes: &["RTU-EXC-CODE"],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::Valid(CaseRequest::ReadHolding { addr: 0, count: 0 }),
            expect: Expected::Exception { code: 0x03 },
        }],
    },
    Case {
        id: "WRITE-SINGLE-READBACK",
        description: "FC06 echoes and persists the written value",
        probes: &["RTU-RESP-FC", "RTU-MASTER-WAIT"],
        judge_master_dir: true,
        steps: &[
            Step {
                stimulus: Stimulus::Valid(CaseRequest::WriteSingle {
                    addr: 5,
                    value: 0xBEEF,
                }),
                expect: Expected::EchoWriteSingle {
                    addr: 5,
                    value: 0xBEEF,
                },
            },
            Step {
                stimulus: Stimulus::Valid(CaseRequest::ReadHolding { addr: 5, count: 1 }),
                expect: Expected::ReadValues { values: &[0xBEEF] },
            },
        ],
    },
    Case {
        id: "WRITE-SINGLE-BAD-ADDR",
        description: "FC06 outside the register space yields exception 02",
        probes: &["RTU-EXC-CODE"],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::Valid(CaseRequest::WriteSingle {
                addr: 500,
                value: 1,
            }),
            expect: Expected::Exception { code: 0x02 },
        }],
    },
    Case {
        id: "WRITE-MULTI-READBACK",
        description: "FC16 echoes quantity and persists all values",
        probes: &["RTU-PDU-FORM", "RTU-MASTER-WAIT"],
        judge_master_dir: true,
        steps: &[
            Step {
                stimulus: Stimulus::Valid(CaseRequest::WriteMultiple {
                    addr: 10,
                    values: &[0x1111, 0x2222, 0x3333, 0x4444],
                }),
                expect: Expected::EchoWriteMultiple { addr: 10, count: 4 },
            },
            Step {
                stimulus: Stimulus::Valid(CaseRequest::ReadHolding { addr: 10, count: 4 }),
                expect: Expected::ReadValues {
                    values: &[0x1111, 0x2222, 0x3333, 0x4444],
                },
            },
        ],
    },
    Case {
        id: "UNKNOWN-FC",
        description: "Out-of-subset function code yields exception 01",
        probes: &["RTU-EXC-CODE"],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::UnknownFc { fc: 0x2B },
            expect: Expected::Exception { code: 0x01 },
        }],
    },
    Case {
        id: "WRONG-UNIT-SILENT",
        description: "Requests addressed to another unit are ignored",
        probes: &["RTU-UNSOLICITED"],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::WrongUnit(CaseRequest::ReadHolding { addr: 0, count: 2 }),
            expect: Expected::Silence,
        }],
    },
    Case {
        id: "CRC-CORRUPT-SILENT",
        description: "A CRC-corrupt request is discarded without response",
        probes: &["RTU-UNSOLICITED"],
        judge_master_dir: false,
        steps: &[Step {
            stimulus: Stimulus::CorruptCrc(CaseRequest::ReadHolding { addr: 0, count: 2 }),
            expect: Expected::Silence,
        }],
    },
    Case {
        id: "RUNT-SILENT",
        description: "A sub-minimum fragment is discarded without response",
        probes: &["RTU-FRAME-MIN", "RTU-UNSOLICITED"],
        judge_master_dir: false,
        steps: &[Step {
            stimulus: Stimulus::Runt,
            expect: Expected::Silence,
        }],
    },
    Case {
        id: "SPLIT-FRAME-SILENT",
        description: "A request torn by a > t3.5 gap must not elicit a response",
        probes: &["RTU-T35-FRAME", "RTU-UNSOLICITED"],
        judge_master_dir: false,
        steps: &[Step {
            stimulus: Stimulus::MidFrameGap {
                req: CaseRequest::ReadHolding { addr: 0, count: 2 },
                after_byte: 2,
                gap: GapKind::WellOverT35,
            },
            expect: Expected::Silence,
        }],
    },
    Case {
        id: "GAP-T15-ACCEPTED",
        description: "An exactly-t1.5 intra-frame gap is legal; the DUT must answer",
        probes: &["RTU-T15-INTRA"],
        judge_master_dir: true,
        steps: &[Step {
            stimulus: Stimulus::MidFrameGap {
                req: CaseRequest::ReadHolding { addr: 0, count: 2 },
                after_byte: 2,
                gap: GapKind::ExactlyT15,
            },
            expect: Expected::ReadPattern { addr: 0, count: 2 },
        }],
    },
];
