//! LIN conformance suite (risk T10), mirroring the I2C structure: `no_std`
//! case definitions here, simulator-hosted runner behind the `runner`
//! feature. DUT model: the reference schedule-table publisher
//! (`stablehal_lin::TableSlave`).
//!
//! Unlike MODBUS/I2C, every case here scripts a *conforming* device (the
//! `bugs` field is present for symmetry with the other suites but every
//! `STANDARD_SUITE` case uses the all-`false` default) — bug-catching, per
//! clause, is already exercised with exact-clause assertions by the DST
//! closed-loop test (`crates/stablehal-sim/tests/lin_dst.rs`); this suite's
//! job is wire-contract coverage of the conforming path, mirroring how
//! `stablehal_conformance::i2c` divides the same responsibility.

#[cfg(feature = "runner")]
pub mod runner;

use stablehal_lin::LinSlaveBugs;

/// Frame ID with a 2-data-byte publisher registered on the DUT.
pub const DUT_ID_TWO_BYTE: u8 = 0x10;
/// Frame ID with a 1-data-byte publisher registered on the DUT.
pub const DUT_ID_ONE_BYTE: u8 = 0x05;
/// Frame ID with the maximum 8-data-byte publisher registered on the DUT.
pub const DUT_ID_MAX_DATA: u8 = 0x20;
/// Frame ID with no publisher anywhere on the bus — must go unanswered.
pub const UNASSIGNED_ID: u8 = 0x3A;

/// One conformance case: a fixed scheduled frame plus the clause(s) it
/// probes, for `Report::probed_clauses`-style coverage accounting.
#[derive(Copy, Clone, Debug)]
pub struct Case {
    pub id: &'static str,
    pub description: &'static str,
    pub probes: &'static [&'static str],
    /// DUT bugs to plant for this case (default: conforming device).
    pub bugs: LinSlaveBugs,
}

const CONFORMING: LinSlaveBugs = LinSlaveBugs {
    corrupt_checksum: false,
    respond_to_unowned: None,
    silent_on_owned: false,
};

pub const STANDARD_SUITE: &[Case] = &[
    Case {
        id: "LIN-BASIC-RESPONSE",
        description: "Scheduled read of a 2-byte publisher: header + response must be \
                       well-formed, on time, with a matching checksum.",
        probes: &["LIN-CHECKSUM", "LIN-FRAME-MAXLEN", "LIN-INTERBYTE-MAX"],
        bugs: CONFORMING,
    },
    Case {
        id: "LIN-SINGLE-BYTE-RESPONSE",
        description: "Scheduled read of a 1-byte publisher: the minimum response length \
                       must still complete within its (smaller) frame budget.",
        probes: &["LIN-CHECKSUM", "LIN-FRAME-MAXLEN"],
        bugs: CONFORMING,
    },
    Case {
        id: "LIN-MAX-DATA-RESPONSE",
        description: "Scheduled read of an 8-byte publisher (the maximum response length): \
                       every inter-byte gap and the overall frame budget must hold at the \
                       upper size boundary.",
        probes: &["LIN-CHECKSUM", "LIN-FRAME-MAXLEN", "LIN-INTERBYTE-MAX"],
        bugs: CONFORMING,
    },
    Case {
        id: "LIN-UNASSIGNED-SILENT",
        description: "Scheduled header for a frame ID with no registered publisher on the \
                       bus: silence is legal, not a breach.",
        probes: &["LIN-SLAVE-SILENCE"],
        bugs: CONFORMING,
    },
];
