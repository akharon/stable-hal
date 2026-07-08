//! I2C conformance suite (WP2 §2.2.5), mirroring the MODBUS structure: `no_std`
//! case definitions here, simulator-hosted runner behind the `runner` feature.
//! DUT model: the reference register-pointer device
//! (`stablehal_i2c::RegisterPointerDevice`).

#[cfg(feature = "runner")]
pub mod runner;

use stablehal_i2c::I2cSlaveBugs;

/// The address the DUT must be configured at.
pub const DUT_ADDR: u8 = 0x50;
/// A neighboring, unconfigured address — must NACK.
pub const OTHER_ADDR: u8 = 0x51;

/// One conformance case: a fixed scripted stimulus plus the clause(s) it
/// probes, for `Report::probed_clauses`-style coverage accounting.
#[derive(Copy, Clone, Debug)]
pub struct Case {
    pub id: &'static str,
    pub description: &'static str,
    pub probes: &'static [&'static str],
    /// DUT bugs to plant for this case (default: conforming device).
    pub bugs: I2cSlaveBugs,
}

const CONFORMING: I2cSlaveBugs = I2cSlaveBugs {
    stretch_over_cap: None,
    nack_own_addr: false,
    wrong_read_data: false,
};

pub const STANDARD_SUITE: &[Case] = &[
    Case {
        id: "I2C-WRITE-READBACK",
        description: "Write [ptr, 0xAB, 0xCD] then re-point and read 2 bytes back: must \
                       return the written bytes.",
        probes: &["I2C-SEQ", "I2C-WRITE-ACK", "I2C-READ-NACK-LAST"],
        bugs: CONFORMING,
    },
    Case {
        id: "I2C-POINTER-WRAP",
        description: "Write at ptr=255 then read 2 bytes without re-pointing: pointer must \
                       wrap 255 -> 0.",
        probes: &["I2C-SEQ"],
        bugs: CONFORMING,
    },
    Case {
        id: "I2C-ADDR-ACK",
        description: "Probe the DUT's own configured address: must ACK.",
        probes: &["I2C-ADDR-ACK"],
        bugs: CONFORMING,
    },
    Case {
        id: "I2C-OTHER-ADDR-SILENT",
        description: "Probe a neighboring, unconfigured address: must NACK (no device \
                       present at that address).",
        probes: &["I2C-ADDR-ACK"],
        bugs: CONFORMING,
    },
    Case {
        id: "I2C-STRETCH-WITHIN-CAP",
        description: "DUT stretches every byte just under stretch_max: must still pass.",
        probes: &["I2C-STRETCH-MAX"],
        bugs: CONFORMING,
    },
];
