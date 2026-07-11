//! Reference SPI slave: a 256-byte register-pointer device, the same
//! first-byte-is-pointer idiom as `stablehal_i2c::RegisterPointerDevice`.
//! Also the DUT model for the conformance suite.

use crate::{SpiMode, SpiSlave};

/// All-`false` is a conforming device — mirrors `stablehal_i2c`'s
/// `I2cSlaveBugs`/`stablehal_lin`'s `LinSlaveBugs` catalog design.
///
/// SPI's fully-synchronous byte exchange gives a device no hook to
/// misbehave on CS timing, mode discipline, or transfer timing — those are
/// master/world-level obligations (see the crate-level doc comment). Data
/// correctness is the one obligation a device controls.
#[derive(Copy, Clone, Debug, Default)]
pub struct SpiSlaveBugs {
    /// Return a fixed wrong byte on every read instead of the pointed-to
    /// register value (a data-correctness bug; not itself a wire-contract
    /// breach — mirrors `I2cSlaveBugs::wrong_read_data`).
    pub wrong_read_data: bool,
}

/// A minimal SPI slave: 256 byte register file, addressed by the first byte
/// of the transfer as the register offset; subsequent bytes read/write
/// sequentially with auto-increment.
pub struct SpiRegisterDevice {
    #[allow(dead_code)]
    addr: u8, // chip-select address
    regs: [u8; 256],
    ptr: u8,
    selected: bool,
    byte_idx: u8,
    bugs: SpiSlaveBugs,
}

impl SpiRegisterDevice {
    pub fn new(addr: u8) -> Self {
        SpiRegisterDevice {
            addr,
            regs: [0u8; 256],
            ptr: 0,
            selected: false,
            byte_idx: 0,
            bugs: SpiSlaveBugs::default(),
        }
    }

    pub fn with_bugs(addr: u8, bugs: SpiSlaveBugs) -> Self {
        SpiRegisterDevice {
            bugs,
            ..SpiRegisterDevice::new(addr)
        }
    }

    pub fn read_reg(&self, offset: u8) -> u8 {
        self.regs[offset as usize]
    }

    pub fn write_reg(&mut self, offset: u8, val: u8) {
        self.regs[offset as usize] = val;
    }
}

impl SpiSlave for SpiRegisterDevice {
    fn on_select(&mut self, _mode: SpiMode) {
        self.selected = true;
        self.byte_idx = 0;
    }

    fn xfer_byte(&mut self, mosi: u8) -> u8 {
        if self.byte_idx == 0 {
            // First byte: register pointer.
            self.ptr = mosi;
            self.byte_idx += 1;
            let val = self.regs[self.ptr as usize];
            if self.bugs.wrong_read_data {
                !val
            } else {
                val
            }
        } else {
            // Subsequent bytes: write then auto-increment read.
            let prev = self.regs[self.ptr as usize];
            self.regs[self.ptr as usize] = mosi;
            self.ptr = self.ptr.wrapping_add(1);
            self.byte_idx += 1;
            if self.bugs.wrong_read_data {
                !prev
            } else {
                prev
            }
        }
    }

    fn on_deselect(&mut self) {
        self.selected = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_device_basic_write_read() {
        let mut dev = SpiRegisterDevice::new(0);
        dev.write_reg(0x10, 0xAB);

        dev.on_select(SpiMode::Mode0);
        // First byte: set pointer to 0x10, read returns 0xAB.
        assert_eq!(dev.xfer_byte(0x10), 0xAB);
        // Second byte: write 0xCD to regs[0x10], return old value (0xAB).
        assert_eq!(dev.xfer_byte(0xCD), 0xAB);
        // Read back: pointer now at 0x11, write 0xFF, return old value.
        assert_eq!(dev.xfer_byte(0xFF), 0x00);
        dev.on_deselect();

        assert_eq!(dev.read_reg(0x10), 0xCD);
        assert_eq!(dev.read_reg(0x11), 0xFF);
    }

    #[test]
    fn wrong_read_data_bug_inverts_every_returned_byte() {
        let mut dev = SpiRegisterDevice::with_bugs(
            0,
            SpiSlaveBugs {
                wrong_read_data: true,
            },
        );
        dev.write_reg(0x10, 0xAB);
        dev.on_select(SpiMode::Mode0);
        assert_eq!(dev.xfer_byte(0x10), !0xABu8);
        dev.on_deselect();
    }
}
