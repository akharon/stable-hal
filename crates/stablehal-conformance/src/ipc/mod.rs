//! IPC conformance suite, mirroring the SPI/I2C structure: `no_std` case
//! definitions here, simulator-hosted runner behind the `runner` feature.
//! DUT model: the reference responder (`stablehal_ipc::IpcResponderDevice`).
//!
//! Unlike the SPI suite — where every case is conforming by construction
//! because a slave has no hook to misbehave — an IPC responder controls most
//! of what it is judged on, so this suite has real negative cases: a
//! responder that answers the wrong correlation id, one that never answers,
//! and one that stalls past the scheduling envelope. Each names the single
//! clause it must be caught by.

#[cfg(feature = "runner")]
pub mod runner;

use stablehal_ipc::IpcResponderBugs;

/// One conformance case: a scripted stimulus plus the clause(s) it probes.
#[derive(Copy, Clone, Debug)]
pub struct Case {
    pub id: &'static str,
    pub description: &'static str,
    pub probes: &'static [&'static str],
    /// DUT bugs to plant for this case (default: conforming responder).
    pub bugs: IpcResponderBugs,
    /// The clause this case must be caught by, or `None` if the case is
    /// expected to pass. Making the expectation explicit is what stops a
    /// case from "passing" because the harness silently did nothing.
    pub expect_breach: Option<&'static str>,
    /// Pre-occupy this many queue slots to force backpressure.
    pub background_load: u16,
}

const CONFORMING: IpcResponderBugs = IpcResponderBugs {
    wrong_correlation: false,
    never_answer: false,
    answer_twice: false,
    oversize_response: false,
    slow_response: false,
    wrong_payload: false,
};

pub const STANDARD_SUITE: &[Case] = &[
    Case {
        id: "IPC-BASIC-EXCHANGE",
        description: "A single request/response exchange must complete in order, under \
                      the same correlation id, inside the delivery and scheduling \
                      envelopes.",
        probes: &["IPC-SEQ", "IPC-CORRELATION", "IPC-DELIVERY"],
        bugs: CONFORMING,
        expect_breach: None,
        background_load: 0,
    },
    Case {
        id: "IPC-SCHEDULING-ENVELOPE",
        description: "A sequence of exchanges across the full non-real-time scheduling \
                      envelope: every turnaround must stay inside resp_time.",
        probes: &["IPC-RESP-TIME"],
        bugs: CONFORMING,
        expect_breach: None,
        background_load: 0,
    },
    Case {
        id: "IPC-BACKPRESSURE-RETRY",
        description: "With the receive queue pre-filled, sends must be rejected loudly \
                      and retried — never silently dropped.",
        probes: &["IPC-BACKPRESSURE"],
        bugs: CONFORMING,
        expect_breach: None,
        background_load: 8,
    },
    Case {
        id: "IPC-WRONG-CORRELATION",
        description: "A responder that answers under the wrong correlation id must be \
                      caught by IPC-CORRELATION.",
        probes: &["IPC-CORRELATION"],
        bugs: IpcResponderBugs {
            wrong_correlation: true,
            ..CONFORMING
        },
        expect_breach: Some("IPC-CORRELATION"),
        background_load: 0,
    },
    Case {
        id: "IPC-SILENT-HANG",
        description: "A responder that never answers must be caught by IPC-RESP-TIME at \
                      end of observation, not left to hang.",
        probes: &["IPC-RESP-TIME", "IPC-PEER-DEATH"],
        bugs: IpcResponderBugs {
            never_answer: true,
            ..CONFORMING
        },
        expect_breach: Some("IPC-RESP-TIME"),
        background_load: 0,
    },
];
