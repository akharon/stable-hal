//! Linux SPI tester: drives a scripted transaction with the unmodified
//! `ScriptedSpiMaster` reference driver against a real `spidev` device.
//!
//! Usage: `linux-spi-tester <spidev-path>` (e.g. `/dev/spidev0.0`)
//!
//! Needs real SPI hardware to be meaningful: either a peripheral wired to
//! the bus, or MOSI physically jumpered to MISO (a wire loopback) so bytes
//! written come back unchanged — there is no equivalent of the UART port's
//! PTY loopback for spidev (the kernel node talks to real clocked hardware,
//! no virtual/software spidev pair ships in-tree on stock distros). This
//! binary has not been run against real hardware this session (none on
//! hand) — treat it as unverified in the same sense as `targets/rp2040`
//! (T9) until confirmed on a Raspberry Pi / similar with SPI enabled.

use stablehal_linux::spi::SpidevMaster;
use stablehal_spi::{ScriptedSpiMaster, SpiConfig, SpiStep};

const SCRIPT: &[SpiStep] = &[
    SpiStep {
        chip: 0,
        tx: &[0xAB, 0xCD, 0xEF],
    },
    SpiStep {
        chip: 0,
        tx: &[0x01, 0x02],
    },
];

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: linux-spi-tester <spidev-path>");

    let mut port = SpidevMaster::open(&path, SpiConfig::default_1mhz()).expect("open spidev");
    let mut driver = ScriptedSpiMaster::new(SCRIPT);

    let mut iterations = 0;
    while !driver.done() && iterations < 1_000_000 {
        driver.step(&mut port);
        iterations += 1;
    }

    assert!(driver.done(), "SPI script did not complete");
    for (i, outcome) in driver.outcomes().iter().enumerate() {
        eprintln!("step {i}: {outcome:?}");
    }
}
