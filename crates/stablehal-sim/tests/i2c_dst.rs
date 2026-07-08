//! Closed loop for the I2C contract (WP2 §2.2), mirroring `dst.rs`'s MODBUS
//! closed loop: the reference master + reference register-pointer device
//! must pass the whole envelope; each planted `I2cSlaveBugs` entry must be
//! caught by the oracle with the exact clause named; seed replay must be
//! byte-for-byte exact.

use stablehal_contract::Nanos;
use stablehal_i2c::{I2cSlaveBugs, I2cTiming, RegisterPointerDevice, ScriptedMaster, Step};
use stablehal_sim::i2c::{I2cMasterAdapter, I2cWorld, I2cWorldConfig};

const ADDR: u8 = 0x50;
const DEADLINE: Nanos = Nanos::from_millis(50);

const SCRIPT: &[Step] = &[
    Step::Write {
        addr7: ADDR,
        bytes: &[0x00, 0xAB, 0xCD],
        stop: true,
    },
    Step::Write {
        addr7: ADDR,
        bytes: &[0x00],
        stop: false,
    },
    Step::Read {
        addr7: ADDR,
        len: 2,
        stop: true,
    },
];

fn run(seed: u64, bugs: I2cSlaveBugs) -> (Result<(), String>, u64) {
    let master = ScriptedMaster::new(SCRIPT);
    let mut world = I2cWorld::new(
        I2cWorldConfig::new(seed, I2cTiming::standard()),
        Box::new(I2cMasterAdapter::new(master)),
    );
    world.add_slave(Box::new(RegisterPointerDevice::with_bugs(ADDR, bugs)));
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
        let (outcome, _) = run(seed, I2cSlaveBugs::default());
        assert!(
            outcome.is_ok(),
            "seed {seed} unexpectedly failed: {outcome:?}"
        );
    }
}

#[test]
fn read_after_pointer_write_returns_the_written_bytes() {
    // WRITE-READBACK: after writing [ptr=0, 0xAB, 0xCD], a re-pointered read
    // of 2 bytes from ptr=0 must return [0xAB, 0xCD] — the reference device's
    // core behavior, checked independently of the wire contract.
    let mut device = RegisterPointerDevice::new(ADDR);
    stablehal_i2c::I2cSlaveDevice::on_start(&mut device);
    stablehal_i2c::I2cSlaveDevice::on_write(&mut device, 0x00);
    stablehal_i2c::I2cSlaveDevice::on_write(&mut device, 0xAB);
    stablehal_i2c::I2cSlaveDevice::on_write(&mut device, 0xCD);
    stablehal_i2c::I2cSlaveDevice::on_stop(&mut device);
    stablehal_i2c::I2cSlaveDevice::on_start(&mut device);
    stablehal_i2c::I2cSlaveDevice::on_write(&mut device, 0x00);
    let (b0, _) = stablehal_i2c::I2cSlaveDevice::on_read(&mut device);
    let (b1, _) = stablehal_i2c::I2cSlaveDevice::on_read(&mut device);
    assert_eq!((b0, b1), (0xAB, 0xCD));
}

#[test]
fn nack_own_addr_is_caught_with_exact_clause() {
    let bugs = I2cSlaveBugs {
        nack_own_addr: true,
        ..Default::default()
    };
    let (outcome, _) = run(7, bugs);
    let msg = outcome.expect_err("nack_own_addr must be caught");
    assert!(msg.contains("I2C-ADDR-ACK"), "wrong clause: {msg}");
}

#[test]
fn stretch_over_cap_is_caught_with_exact_clause() {
    let bugs = I2cSlaveBugs {
        stretch_over_cap: Some(I2cTiming::standard().stretch_max + Nanos::from_millis(1)),
        ..Default::default()
    };
    let (outcome, _) = run(3, bugs);
    let msg = outcome.expect_err("stretch_over_cap must be caught");
    assert!(msg.contains("I2C-STRETCH-MAX"), "wrong clause: {msg}");
}

#[test]
fn wrong_read_data_does_not_breach_the_wire_contract() {
    // wrong_read_data is a data-correctness bug, not a wire-protocol breach
    // (per stablehal_i2c::device docs) — the oracle must still pass; only a
    // higher-level content check (conformance suite) catches it.
    let bugs = I2cSlaveBugs {
        wrong_read_data: true,
        ..Default::default()
    };
    let (outcome, _) = run(9, bugs);
    assert!(
        outcome.is_ok(),
        "wire contract must not flag a data bug: {outcome:?}"
    );
}

#[test]
fn seed_replay_is_byte_exact() {
    let (outcome_a, hash_a) = run(4242, I2cSlaveBugs::default());
    let (outcome_b, hash_b) = run(4242, I2cSlaveBugs::default());
    assert_eq!(outcome_a, outcome_b);
    assert_eq!(hash_a, hash_b, "seed replay diverged — nondeterminism leak");
}

#[test]
fn distinct_seeds_explore_distinct_schedules() {
    let (_, h0) = run(0, I2cSlaveBugs::default());
    let (_, h1) = run(1, I2cSlaveBugs::default());
    assert_ne!(h0, h1);
}
