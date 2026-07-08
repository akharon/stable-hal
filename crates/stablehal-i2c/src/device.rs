//! Reference I2C slave: a 256-byte register-pointer device (24Cxx-EEPROM-
//! style subset). `Write [ptr, data...]` sets the pointer then writes
//! sequentially; `Read` returns bytes from the pointer, auto-incrementing;
//! the pointer wraps at 256. Also the DUT model for the conformance suite.

use stablehal_contract::Nanos;

use crate::I2cSlaveDevice;

/// All-`false`/zero is a conforming device — mirrors `stablehal_modbus`'s
/// `SlaveBugs` catalog design: flip exactly one field to plant one bug.
#[derive(Copy, Clone, Debug, Default)]
pub struct I2cSlaveBugs {
    /// NACK bytes that stretch the clock past `stretch_max` instead of
    /// respecting the cap (breaches I2C-STRETCH-MAX).
    pub stretch_over_cap: Option<Nanos>,
    /// NACK our own address instead of ACKing it (breaches I2C-ADDR-ACK).
    pub nack_own_addr: bool,
    /// Return a fixed wrong byte on every read instead of the pointed-to
    /// register value (a data-correctness bug; not itself a wire-contract
    /// breach — caught by the WRITE-READBACK conformance case comparing
    /// expected vs. observed data, not by the oracle).
    pub wrong_read_data: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// First byte of a write is the pointer.
    AwaitingPointer,
    Writing,
    Reading,
}

pub struct RegisterPointerDevice {
    addr7: u8,
    regs: [u8; 256],
    ptr: u8,
    phase: Phase,
    bugs: I2cSlaveBugs,
}

impl RegisterPointerDevice {
    pub fn new(addr7: u8) -> Self {
        RegisterPointerDevice {
            addr7,
            regs: [0u8; 256],
            ptr: 0,
            phase: Phase::Idle,
            bugs: I2cSlaveBugs::default(),
        }
    }

    pub fn with_bugs(addr7: u8, bugs: I2cSlaveBugs) -> Self {
        RegisterPointerDevice {
            bugs,
            ..RegisterPointerDevice::new(addr7)
        }
    }

    pub fn read_reg(&self, ptr: u8) -> u8 {
        self.regs[ptr as usize]
    }
    pub fn write_reg(&mut self, ptr: u8, val: u8) {
        self.regs[ptr as usize] = val;
    }
}

impl I2cSlaveDevice for RegisterPointerDevice {
    fn addr7(&self) -> u8 {
        self.addr7
    }

    fn ack_addr(&self) -> bool {
        !self.bugs.nack_own_addr
    }

    fn on_write(&mut self, byte: u8) -> (bool, Nanos) {
        let stretch = self.bugs.stretch_over_cap.unwrap_or(Nanos::ZERO);
        match self.phase {
            Phase::AwaitingPointer => {
                self.ptr = byte;
                self.phase = Phase::Writing;
                (true, stretch)
            }
            Phase::Writing => {
                self.regs[self.ptr as usize] = byte;
                self.ptr = self.ptr.wrapping_add(1);
                (true, stretch)
            }
            Phase::Idle | Phase::Reading => (true, stretch),
        }
    }

    fn on_read(&mut self) -> (u8, Nanos) {
        self.phase = Phase::Reading;
        let stretch = self.bugs.stretch_over_cap.unwrap_or(Nanos::ZERO);
        let byte = if self.bugs.wrong_read_data {
            !self.regs[self.ptr as usize]
        } else {
            self.regs[self.ptr as usize]
        };
        self.ptr = self.ptr.wrapping_add(1);
        (byte, stretch)
    }

    fn on_start(&mut self) {
        self.phase = Phase::AwaitingPointer;
    }

    fn on_stop(&mut self) {
        self.phase = Phase::Idle;
    }
}
