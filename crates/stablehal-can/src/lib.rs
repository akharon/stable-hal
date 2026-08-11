//! CAN data-link contract (ISO 11898-1): frame field validity, bit-stuffing
//! discipline, CRC, ACK slot, and error-frame rules, expressed as an
//! executable state machine over observable bus events.
//!
//! v1 scope: **Classic CAN 2.0B** (standard + extended identifiers), data
//! frames, remote frames, error frames. CAN FD is explicitly deferred to v2.
//!
//! Modeled at **frame granularity** — the granularity at which software
//! contracts live (controllers handle stuffing/CRC in hardware; drivers see
//! frames). Individual bit-level timing (bit positions, sync segments) is
//! out of scope: that is hardware electrical compliance, same split as the
//! UART line contract vs. physical layer.
//!
//! Risk T8 (closed-loop pilot): CAN has no master/slave — it is a broadcast
//! bus where any node can transmit and every other node ACKs a correctly
//! received frame (dominant bit, driven by any receiver, regardless of ID
//! interest). [`node::CanNodeActor`] models that symmetric topology
//! directly (unlike MODBUS/I2C/LIN's master + N-addressed-slaves shape); a
//! reference tester ([`node::CanScriptedNode`]) and reference responder
//! ([`node::CanRegisterNode`]) are driven by a `stablehal-sim`-hosted
//! `stablehal_sim::can::CanWorld` and a `stablehal_conformance::can`
//! suite, mirroring MODBUS/I2C/LIN/SPI.
//!
//! Raw CAN is broadcast-only with no notion of "request/response" at the
//! data-link layer, so [`node::CanRegisterNode`] models an explicit,
//! documented **application-level convention on top of the raw contract**
//! — analogous to how MODBUS's function codes sit on top of the UART line
//! contract: it replies to a fixed request id with a response id carrying
//! register bytes. That convention is not part of the [`CanContract`]
//! itself and is never mistaken for one in the wire-level clause set below.
//!
//! # Wire events and clauses
//!
//! | Clause | Citation | Obligation |
//! |--------|----------|------------|
//! | CAN-FRAME-START | ISO 11898-1 §10.4 | Every frame begins with a dominant SOF bit. |
//! | CAN-ID-FIELD | ISO 11898-1 §10.4.1 | Identifier field must be valid (11-bit standard or 29-bit extended). |
//! | CAN-RTR-DISC | ISO 11898-1 §10.4.2 | RTR bit distinguishes data from remote frames; a remote frame has zero DLC. |
//! | CAN-CRC | ISO 11898-1 §10.4.3 | CRC field (15 bits + delimiter) must be valid. |
//! | CAN-ACK | ISO 11898-1 §10.4.4 | At least one receiver must ACK a correctly received frame (dominant in ACK slot). |
//! | CAN-EOF | ISO 11898-1 §10.4.5 | End-of-frame field is 7 recessive bits. |
//! | CAN-ERROR-FRAME | ISO 11898-1 §10.5 | Error frame consists of 6 dominant bits (error flag) + 8 recessive delimiter. |
//! | CAN-BIT-STUFFING | ISO 11898-1 §10.3.1 | No more than 5 consecutive bits of the same polarity between SOF and CRC delimiter. |
//! | CAN-OVERLOAD | ISO 11898-1 §10.6 | Overload frame consists of 6 dominant bits + 8 recessive delimiter (deferred). |

#![no_std]
#![deny(unsafe_code)]

pub mod contract;
pub mod node;

pub use contract::{CanContract, CanEvent, CanState};
pub use node::{CanNodeActor, CanNodeBugs, CanRegisterNode, CanScriptStep, CanScriptedNode};

use stablehal_contract::Nanos;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// CAN nominal bit timing (prescaler-agnostic: only the nominal bit time in
/// nanoseconds matters at the observable-frame level). The contract checks
/// frame field lengths via bit counting, not wall-clock duration — but the
/// harness can validate total frame time against the baud-rate envelope.
#[derive(Copy, Clone, Debug)]
pub struct CanConfig {
    /// Nominal bit time in nanoseconds (e.g. 1_000 for 1 Mbit/s CAN).
    pub bit_time_ns: Nanos,
    /// Whether extended (29-bit) identifiers are permitted on this bus.
    pub extended_ids: bool,
}

impl CanConfig {
    pub const fn default_500k() -> Self {
        CanConfig {
            bit_time_ns: Nanos::from_nanos(2_000), // 500 kbit/s
            extended_ids: true,
        }
    }

    pub const fn default_1m() -> Self {
        CanConfig {
            bit_time_ns: Nanos::from_nanos(1_000), // 1 Mbit/s
            extended_ids: true,
        }
    }
}

// ---------------------------------------------------------------------------
// CAN identifiers
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanId {
    Standard(u16), // 11-bit, range 0..0x7FF
    Extended(u32), // 29-bit, range 0..0x1FFFFFFF
}

impl CanId {
    pub fn as_raw(&self) -> u32 {
        match *self {
            CanId::Standard(id) => id as u32,
            CanId::Extended(id) => id,
        }
    }
}

// ---------------------------------------------------------------------------
// SW-facing HAL trait
// ---------------------------------------------------------------------------

/// CAN controller state visible to software.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CanControllerState {
    Active,
    ErrorPassive,
    BusOff,
}

/// Errors a CAN controller may report.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CanError {
    StuffError,
    FormError,
    AckError,
    BitError,
    CrcError,
}

/// A CAN frame ready for transmission or received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanFrame {
    pub id: CanId,
    /// Remote transmission request flag: `false` = data frame, `true` = remote frame.
    pub rtr: bool,
    pub data: CanFrameDataBuf,
    pub dlc: u8,
}

/// Fixed-size buffer for CAN data payloads (max 8 bytes for Classic CAN).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanFrameDataBuf {
    buf: [u8; 8],
    len: u8,
}

impl CanFrameDataBuf {
    pub const fn new() -> Self {
        CanFrameDataBuf {
            buf: [0u8; 8],
            len: 0,
        }
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        let mut buf = [0u8; 8];
        let len = slice.len().min(8) as u8;
        buf[..len as usize].copy_from_slice(&slice[..len as usize]);
        CanFrameDataBuf { buf, len }
    }

    /// Create from raw parts (internal use, e.g. from bridge crates).
    #[doc(hidden)]
    pub fn from_raw(buf: [u8; 8], len: u8) -> Self {
        CanFrameDataBuf {
            buf,
            len: len.min(8),
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    pub fn len(&self) -> u8 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for CanFrameDataBuf {
    fn default() -> Self {
        Self::new()
    }
}

/// SW-facing CAN controller HAL trait, poll-driven.
#[allow(clippy::result_unit_err)]
pub trait CanController {
    /// Submit a frame for transmission. Fails if the TX buffer is full.
    fn try_transmit(&mut self, frame: &CanFrame) -> Result<(), ()>;
    /// Poll for a received frame.
    fn try_receive(&mut self) -> Option<Result<CanFrame, CanError>>;
    /// Current controller state (for bus-off detection).
    fn state(&self) -> CanControllerState;
}

// ---------------------------------------------------------------------------
// Reference node model (pre-T8: a minimal driver-facing controller, kept
// for compatibility — see `node` for the world-facing `CanNodeActor` model
// used by the DST closed loop).
// ---------------------------------------------------------------------------

/// A minimal CAN node: a register-like state machine that responds to
/// remote frames with a fixed data payload.
pub struct CanNode {
    pub id: CanId,
    /// Whether this node is `ErrorPassive` and may not drive dominant bits.
    pub error_passive: bool,
    /// Stored data payload, indexed by a function-code byte.
    data: [u8; 256],
}

impl CanNode {
    pub fn new(id: CanId) -> Self {
        CanNode {
            id,
            error_passive: false,
            data: [0u8; 256],
        }
    }

    pub fn set_data(&mut self, offset: u8, val: u8) {
        self.data[offset as usize] = val;
    }

    pub fn get_data(&self, offset: u8) -> u8 {
        self.data[offset as usize]
    }

    pub fn produce_data_frame(&self, func: u8) -> CanFrame {
        // Provide 4 bytes starting at the function-code offset.
        let slice = &self.data[func as usize..(func as usize + 4).min(256)];
        let buf = CanFrameDataBuf::from_slice(slice);
        CanFrame {
            id: self.id,
            rtr: false,
            data: buf,
            dlc: 4,
        }
    }
}

impl CanController for CanNode {
    fn try_transmit(&mut self, frame: &CanFrame) -> Result<(), ()> {
        // In a real node, the response depends on the frame content.
        // Here we just accept it.
        let _ = frame;
        Ok(())
    }

    fn try_receive(&mut self) -> Option<Result<CanFrame, CanError>> {
        // No incoming frames in this simple model.
        None
    }

    fn state(&self) -> CanControllerState {
        if self.error_passive {
            CanControllerState::ErrorPassive
        } else {
            CanControllerState::Active
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_node_produces_data_frame() {
        let mut node = CanNode::new(CanId::Standard(0x100));
        node.set_data(0x20, 0xAA);
        node.set_data(0x21, 0xBB);
        node.set_data(0x22, 0xCC);
        node.set_data(0x23, 0xDD);

        let frame = node.produce_data_frame(0x20);
        assert_eq!(frame.dlc, 4);
        assert_eq!(frame.data.as_slice(), &[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(frame.id, CanId::Standard(0x100));
        assert!(!frame.rtr);
    }
}
