//! Linux I2C tester: drives a scripted transaction with the unmodified
//! `ScriptedMaster` reference driver against a real `i2c-dev` bus.
//!
//! Usage: `linux-i2c-tester <i2c-bus-path> <addr7-hex>`
//! (e.g. `linux-i2c-tester /dev/i2c-1 0x50` for an EEPROM at 0x50)
//!
//! Needs a real I2C device on the bus to be meaningful — unverified against
//! real hardware this session (none on hand); treat like `targets/rp2040`
//! (T9) until confirmed against a real I2C peripheral.

use stablehal_i2c::{ScriptedMaster, Step};
use stablehal_linux::i2c::I2cDevMaster;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: linux-i2c-tester <i2c-bus-path> <addr7-hex>");
    let addr_str = args
        .next()
        .expect("usage: linux-i2c-tester <i2c-bus-path> <addr7-hex>");
    let addr7 = u8::from_str_radix(addr_str.trim_start_matches("0x"), 16).expect("addr7 must be hex, e.g. 0x50");

    let script: &'static [Step] = Box::leak(Box::new([
        Step::Write {
            addr7,
            bytes: &[0x00],
            stop: false,
        },
        Step::Read {
            addr7,
            len: 4,
            stop: true,
        },
    ]));

    let mut port = I2cDevMaster::open(&path).expect("open i2c-dev bus");
    let mut driver = ScriptedMaster::new(script);

    let mut iterations = 0;
    while !driver.done() && iterations < 1_000_000 {
        driver.step(&mut port);
        iterations += 1;
    }

    assert!(driver.done(), "I2C script did not complete");
    for (i, outcome) in driver.outcomes().iter().enumerate() {
        eprintln!("step {i}: {outcome:?}");
    }
}
