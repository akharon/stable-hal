//! CAN conformance suite (risk T8), mirroring the SPI structure: `no_std`
//! case definitions here, simulator-hosted runner behind the `runner`
//! feature. DUT model: the reference request/response responder
//! (`stablehal_can::node::CanRegisterNode`).
//!
//! Only clauses reachable through a `CanEvent::Frame` observation in a
//! conforming run are probed here: `CAN-ID-FIELD`, `CAN-RTR-DISC`,
//! `CAN-ACK`. `CAN-FRAME-START` is never checked in `CanContract::step` at
//! all (its own comment: SOF presence is "assumed for any valid frame
//! observation") and `CAN-EOF`/`CAN-CRC`/`CAN-ERROR-FRAME`/`CAN-OVERLOAD`/
//! `CAN-BIT-STUFFING` require bus-monitor-level events
//! (`FormError`/`CrcError`/`ErrorFrame`/`OverloadFrame`) this reference
//! world never emits — same honesty pattern as SPI's `SPI-DATA-VALID`.

#[cfg(feature = "runner")]
pub mod runner;

use stablehal_can::node::CanNodeBugs;

/// One conformance case: a fixed scripted stimulus plus the clause(s) it
/// probes, for `Report::probed_clauses`-style coverage accounting.
#[derive(Copy, Clone, Debug)]
pub struct Case {
    pub id: &'static str,
    pub description: &'static str,
    pub probes: &'static [&'static str],
    /// DUT bugs to plant for this case (default: conforming device).
    pub bugs: CanNodeBugs,
}

const CONFORMING: CanNodeBugs = CanNodeBugs {
    wrong_response_data: false,
    silent_no_ack: false,
    silent_no_response: false,
};

pub const STANDARD_SUITE: &[Case] = &[
    Case {
        id: "CAN-DATA-FRAME-BASIC",
        description: "A standard-id data frame request must be observed with a valid id \
                       field and ACKed by the responder.",
        probes: &["CAN-ID-FIELD", "CAN-ACK"],
        bugs: CONFORMING,
    },
    Case {
        id: "CAN-REMOTE-FRAME",
        description: "A remote frame (RTR set, zero data bytes) must be observed as legal \
                       and ACKed, not flagged as an RTR-discipline violation.",
        probes: &["CAN-RTR-DISC"],
        bugs: CONFORMING,
    },
    Case {
        id: "CAN-EXTENDED-ID",
        description: "An extended (29-bit) id data frame request must be observed as a \
                       valid id field on a bus configured for extended ids.",
        probes: &["CAN-ID-FIELD"],
        bugs: CONFORMING,
    },
];
