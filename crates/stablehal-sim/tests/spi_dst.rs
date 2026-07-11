//! Closed loop for the SPI contract (risk T8), mirroring `i2c_dst.rs`'s
//! structure: the reference master + reference register device must pass
//! the whole envelope; seed replay must be byte-for-byte exact. SPI's
//! `SpiSlaveBugs::wrong_read_data` is a data-correctness bug, not a
//! wire-contract breach (mirrors `I2cSlaveBugs::wrong_read_data`) — see the
//! crate-level doc comment on `stablehal_spi` for why the wire-level clauses
//! (CS timing, mode discipline, transfer timing) have no device-side bug
//! hook and are instead negative-tested in `stablehal_spi::contract`'s own
//! `Oracle`-driven unit tests.

use stablehal_contract::Nanos;
use stablehal_sim::spi::{SpiMasterAdapter, SpiWorld, SpiWorldConfig};
use stablehal_spi::{ScriptedSpiMaster, SpiConfig, SpiRegisterDevice, SpiSlaveBugs, SpiStep};

const CHIP: u8 = 0;
const DEADLINE: Nanos = Nanos::from_millis(50);

const SCRIPT: &[SpiStep] = &[
    SpiStep {
        chip: CHIP,
        tx: &[0x00, 0xAB, 0xCD],
    },
    SpiStep {
        chip: CHIP,
        tx: &[0x00, 0x00, 0x00],
    },
];

fn run(seed: u64, bugs: SpiSlaveBugs) -> (Result<(), String>, u64) {
    let master = ScriptedSpiMaster::new(SCRIPT);
    let mut world = SpiWorld::new(
        SpiWorldConfig::new(seed, SpiConfig::default_1mhz()),
        Box::new(SpiMasterAdapter::new(master)),
    );
    world.add_slave(CHIP, Box::new(SpiRegisterDevice::with_bugs(CHIP, bugs)));
    let outcome = world
        .run_until(DEADLINE)
        .map(|_| ())
        .map_err(|e| e.to_string());
    let hash = world.trace_hash();
    (outcome, hash)
}

#[test]
fn reference_master_and_device_survive_the_whole_envelope() {
    for seed in 0..50 {
        let (outcome, _) = run(seed, SpiSlaveBugs::default());
        assert!(
            outcome.is_ok(),
            "seed {seed} unexpectedly failed: {outcome:?}"
        );
    }
}

#[test]
fn read_after_pointer_write_returns_the_written_bytes() {
    // Mirrors I2C's own data-correctness test: hit the device directly,
    // independent of the wire contract (see stablehal_spi::device's own
    // `register_device_basic_write_read` for the same check, kept here too
    // so the DST closed-loop file documents the property end to end).
    let mut dev = SpiRegisterDevice::new(CHIP);
    dev.write_reg(0x00, 0x00); // no-op, just documents the pointer byte
    stablehal_spi::SpiSlave::on_select(&mut dev, stablehal_spi::SpiMode::Mode0);
    let _ = stablehal_spi::SpiSlave::xfer_byte(&mut dev, 0x10); // pointer <- 0x10
    stablehal_spi::SpiSlave::xfer_byte(&mut dev, 0xAB); // write 0xAB at 0x10
    stablehal_spi::SpiSlave::xfer_byte(&mut dev, 0xCD); // write 0xCD at 0x11
    stablehal_spi::SpiSlave::on_deselect(&mut dev);

    assert_eq!(dev.read_reg(0x10), 0xAB);
    assert_eq!(dev.read_reg(0x11), 0xCD);
}

#[test]
fn wrong_read_data_does_not_breach_the_wire_contract() {
    let bugs = SpiSlaveBugs {
        wrong_read_data: true,
    };
    let (outcome, _) = run(1, bugs);
    assert!(
        outcome.is_ok(),
        "a data-correctness bug must not breach the wire contract: {outcome:?}"
    );
}

#[test]
fn seed_replay_is_byte_exact() {
    let (outcome_a, hash_a) = run(4242, SpiSlaveBugs::default());
    let (outcome_b, hash_b) = run(4242, SpiSlaveBugs::default());
    assert!(outcome_a.is_ok());
    assert_eq!(outcome_a.is_ok(), outcome_b.is_ok());
    assert_eq!(hash_a, hash_b);
}

#[test]
fn distinct_seeds_explore_distinct_schedules() {
    let (_, hash_a) = run(1, SpiSlaveBugs::default());
    let (_, hash_b) = run(2, SpiSlaveBugs::default());
    assert_ne!(hash_a, hash_b);
}
