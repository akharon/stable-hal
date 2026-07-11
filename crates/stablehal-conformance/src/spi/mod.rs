//! SPI conformance suite (risk T8), mirroring the I2C structure: `no_std`
//! case definitions here, simulator-hosted runner behind the `runner`
//! feature. DUT model: the reference register-pointer device
//! (`stablehal_spi::SpiRegisterDevice`).
//!
//! Every case here is conforming by construction: SPI's fully-synchronous
//! byte exchange gives a device no hook to misbehave on CS timing, mode
//! discipline, or transfer timing (see `stablehal_spi`'s crate-level doc
//! comment) — those clauses are exercised implicitly by every case rather
//! than probed individually via a device bug catalog, mirroring how the I2C
//! suite treats `I2C-BUS-FREE`/`I2C-BYTE-TIME`.

#[cfg(feature = "runner")]
pub mod runner;

use stablehal_spi::SpiSlaveBugs;

/// The chip-select id the DUT answers to.
pub const DUT_CHIP: u8 = 0;

/// One conformance case: a fixed scripted stimulus plus the clause(s) it
/// probes, for `Report::probed_clauses`-style coverage accounting.
#[derive(Copy, Clone, Debug)]
pub struct Case {
    pub id: &'static str,
    pub description: &'static str,
    pub probes: &'static [&'static str],
    /// DUT bugs to plant for this case (default: conforming device).
    pub bugs: SpiSlaveBugs,
}

const CONFORMING: SpiSlaveBugs = SpiSlaveBugs {
    wrong_read_data: false,
};

pub const STANDARD_SUITE: &[Case] = &[
    Case {
        id: "SPI-BASIC-TRANSFER",
        description: "A single 3-byte transfer (pointer + 2 data bytes) must complete as \
                       one well-formed transaction: one CS assert, matching mode, one CS \
                       de-assert.",
        probes: &["SPI-TRANSACTION", "SPI-CS-TIMING", "SPI-MODE-DISC"],
        bugs: CONFORMING,
    },
    Case {
        id: "SPI-MULTI-BYTE-TRANSFER",
        description: "An 8-byte transfer: every byte's cumulative elapsed time must stay \
                       inside the SCLK-period envelope.",
        probes: &["SPI-XFER-TIME"],
        bugs: CONFORMING,
    },
    Case {
        id: "SPI-BACK-TO-BACK-TRANSFERS",
        description: "Two transfers to the same chip in sequence: the gap between CS \
                       de-assert and the next CS assert must respect cs_idle_min.",
        probes: &["SPI-CS-MIN-IDLE"],
        bugs: CONFORMING,
    },
];
