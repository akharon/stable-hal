//! Reference LIN slave: a fixed-table signal publisher/subscriber. Each
//! frame ID this device owns publishes `n_data` bytes from an internal
//! register file (set by the application, e.g. a sensor reading); frames
//! the device does not own are ignored, matching real LIN node behavior
//! where only the assigned publisher drives a given frame's response.

use crate::contract::enhanced_checksum;
use crate::LinSlaveDevice;

/// All-`false` is a conforming device — mirrors `stablehal_i2c`'s
/// `I2cSlaveBugs` / `stablehal_modbus`'s `SlaveBugs` catalog design: flip
/// exactly one field to plant one bug.
#[derive(Copy, Clone, Debug, Default)]
pub struct LinSlaveBugs {
    /// Emit a checksum that does not match the response data (breaches
    /// LIN-CHECKSUM).
    pub corrupt_checksum: bool,
    /// Respond to a frame ID this device does not own (breaches
    /// LIN-SLAVE-SILENCE / LIN-ONE-RESPONSE from the bus's point of view).
    pub respond_to_unowned: Option<u8>,
    /// Stay silent on a frame ID this device *does* own (breaches
    /// LIN-SLAVE-SILENCE).
    pub silent_on_owned: bool,
}

/// One published signal: an `id` this device answers, with fixed-size data.
#[derive(Copy, Clone, Debug)]
pub struct PublishedFrame {
    pub id: u8,
    pub data: [u8; 8],
    pub len: u8,
}

const MAX_FRAMES: usize = 8;

pub struct TableSlave {
    frames: [Option<PublishedFrame>; MAX_FRAMES],
    bugs: LinSlaveBugs,
}

impl TableSlave {
    pub fn new(frames: &[PublishedFrame]) -> Self {
        let mut table = [None; MAX_FRAMES];
        for (slot, &f) in table.iter_mut().zip(frames.iter()) {
            *slot = Some(f);
        }
        TableSlave {
            frames: table,
            bugs: LinSlaveBugs::default(),
        }
    }

    pub fn with_bugs(frames: &[PublishedFrame], bugs: LinSlaveBugs) -> Self {
        TableSlave {
            bugs,
            ..TableSlave::new(frames)
        }
    }

    /// Update the data this device publishes for `id`, if it owns that ID.
    pub fn set_data(&mut self, id: u8, data: &[u8]) {
        for slot in self.frames.iter_mut().flatten() {
            if slot.id == id {
                let len = data.len().min(8);
                slot.data[..len].copy_from_slice(&data[..len]);
                slot.len = len as u8;
            }
        }
    }

    fn owns(&self, id: u8) -> Option<PublishedFrame> {
        self.frames.iter().flatten().find(|f| f.id == id).copied()
    }
}

impl LinSlaveDevice for TableSlave {
    fn respond(&mut self, pid: u8, id: u8) -> Option<([u8; 8], u8, u8)> {
        if let Some(forced_id) = self.bugs.respond_to_unowned {
            if id == forced_id && self.owns(id).is_none() {
                let data = [0u8; 8];
                let checksum = enhanced_checksum(pid, &data[..1]);
                return Some((data, 1, checksum));
            }
        }
        let frame = self.owns(id)?;
        if self.bugs.silent_on_owned {
            return None;
        }
        let checksum = enhanced_checksum(pid, &frame.data[..frame.len as usize]);
        let checksum = if self.bugs.corrupt_checksum {
            checksum.wrapping_add(1)
        } else {
            checksum
        };
        Some((frame.data, frame.len, checksum))
    }
}
