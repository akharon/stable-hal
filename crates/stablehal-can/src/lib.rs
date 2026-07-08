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

use stablehal_contract::{Breach, Clause, Contract, Nanos};

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
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
// Wire-facing contract
// ---------------------------------------------------------------------------

/// Observable events on the CAN bus (frame-level, not bit-level).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanEvent {
    /// A frame was correctly transmitted/received: header (id + flags) and
    /// payload, with the observed ACK state.
    Frame {
        id: CanId,
        rtr: bool,
        data: CanFrameDataBuf,
        acked: bool,
    },
    /// A frame with a CRC error was observed on the bus.
    CrcError,
    /// A frame with a form error was observed on the bus.
    FormError,
    /// An error frame (6 dominant + 8 recessive) was observed.
    ErrorFrame,
    /// An overload frame was observed.
    OverloadFrame,
    /// Appended by the harness.
    EndOfObservation,
}

pub static CL_CAN_FRAME_START: Clause = Clause {
    id: "CAN-FRAME-START",
    citation: "ISO 11898-1 §10.4",
    text: "Every frame begins with a dominant SOF bit.",
};

pub static CL_CAN_ID_FIELD: Clause = Clause {
    id: "CAN-ID-FIELD",
    citation: "ISO 11898-1 §10.4.1",
    text:
        "Identifier field must be valid (11-bit standard or 29-bit extended per bus configuration).",
};

pub static CL_CAN_RTR_DISC: Clause = Clause {
    id: "CAN-RTR-DISC",
    citation: "ISO 11898-1 §10.4.2",
    text: "RTR bit distinguishes data frames (dominant) from remote frames (recessive); remote frames carry zero data bytes.",
};

pub static CL_CAN_CRC: Clause = Clause {
    id: "CAN-CRC",
    citation: "ISO 11898-1 §10.4.3",
    text: "CRC field (15 bits + delimiter) must be valid for the frame content.",
};

pub static CL_CAN_ACK: Clause = Clause {
    id: "CAN-ACK",
    citation: "ISO 11898-1 §10.4.4",
    text: "At least one receiver must ACK a correctly received frame (dominant in ACK slot).",
};

pub static CL_CAN_EOF: Clause = Clause {
    id: "CAN-EOF",
    citation: "ISO 11898-1 §10.4.5",
    text: "End-of-frame field is 7 recessive bits.",
};

pub static CL_CAN_ERROR_FRAME: Clause = Clause {
    id: "CAN-ERROR-FRAME",
    citation: "ISO 11898-1 §10.5",
    text: "Error frame consists of 6 dominant bits (error flag) + 8 recessive delimiter.",
};

pub static CL_CAN_BIT_STUFFING: Clause = Clause {
    id: "CAN-BIT-STUFFING",
    citation: "ISO 11898-1 §10.3.1",
    text: "No more than 5 consecutive bits of the same polarity between SOF and CRC delimiter.",
};

pub static CL_CAN_OVERLOAD: Clause = Clause {
    id: "CAN-OVERLOAD",
    citation: "ISO 11898-1 §10.6",
    text: "Overload frame consists of 6 dominant bits + 8 recessive delimiter.",
};

static CAN_CLAUSES: [&Clause; 9] = [
    &CL_CAN_FRAME_START,
    &CL_CAN_ID_FIELD,
    &CL_CAN_RTR_DISC,
    &CL_CAN_CRC,
    &CL_CAN_ACK,
    &CL_CAN_EOF,
    &CL_CAN_ERROR_FRAME,
    &CL_CAN_BIT_STUFFING,
    &CL_CAN_OVERLOAD,
];

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct CanContract {
    pub config: CanConfig,
    /// Judge the transmitter's obligations.
    pub judge_tx: bool,
    /// Judge receiver ACK obligations.
    pub judge_rx: bool,
}

impl CanContract {
    pub fn new(config: CanConfig) -> Self {
        CanContract {
            config,
            judge_tx: true,
            judge_rx: true,
        }
    }

    fn fail_tx(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_tx {
            Err(breach)
        } else {
            Ok(())
        }
    }

    fn fail_rx(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_rx {
            Err(breach)
        } else {
            Ok(())
        }
    }

    /// Validate an ID field against the bus configuration.
    fn validate_id(&self, id: &CanId) -> Result<(), Breach> {
        match *id {
            CanId::Standard(raw) => {
                if raw > 0x7FF {
                    return Err(Breach::of(&CL_CAN_ID_FIELD));
                }
            }
            CanId::Extended(_raw) => {
                if !self.config.extended_ids {
                    return Err(Breach::of(&CL_CAN_ID_FIELD));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CanState {
    // No per-frame tracking needed at frame-level granularity:
    // each `Frame` event is judged atomically.
}

impl Contract for CanContract {
    type State = CanState;
    type Event = CanEvent;

    fn initial(&self) -> CanState {
        CanState {}
    }

    fn step(&self, _state: &mut CanState, _at: Nanos, event: &CanEvent) -> Result<(), Breach> {
        match event {
            CanEvent::Frame {
                id,
                rtr,
                data,
                acked,
            } => {
                // SOF check: every frame implicitly starts with SOF if we
                // observe a Frame event; a true SOF violation would manifest
                // as a form error or be invisible at frame granularity.
                // At the frame level, SOF presence is assumed for any valid
                // frame observation.

                // ID field validity.
                self.validate_id(id)?;

                // RTR discipline: remote frames must have zero DLC.
                if *rtr && !data.is_empty() {
                    self.fail_tx(Breach::of(&CL_CAN_RTR_DISC))?;
                }

                // ACK check.
                if !*acked {
                    self.fail_rx(Breach::of(&CL_CAN_ACK))?;
                }

                // At frame granularity, we cannot validate CRC or
                // bit-stuffing — those require bit-level observation.
                // Those violations surface as CrcError / FormError events
                // from a bus monitor.

                Ok(())
            }
            CanEvent::CrcError => {
                // CRC error flags a transmitter obligation violation.
                self.fail_tx(Breach::of(&CL_CAN_CRC))
            }
            CanEvent::FormError => {
                // Form error (includes EOF violations, bit-stuffing violations
                // that manifest as form errors).
                self.fail_tx(Breach::of(&CL_CAN_EOF))
            }
            CanEvent::ErrorFrame => {
                // Error frames are legal in CAN — but a transmitter that
                // causes one is in breach of its transmission obligations.
                // At frame-level granularity we just record its occurrence;
                // the specific clause depends on what caused it, surfaced as
                // the preceding CrcError/FormError event.
                self.fail_tx(Breach::of(&CL_CAN_ERROR_FRAME))
            }
            CanEvent::OverloadFrame => self.fail_tx(Breach::of(&CL_CAN_OVERLOAD)),
            CanEvent::EndOfObservation => Ok(()),
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &CAN_CLAUSES
    }
}

// ---------------------------------------------------------------------------
// Reference node model
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::Oracle;

    fn cfg() -> CanConfig {
        CanConfig::default_500k()
    }

    #[test]
    fn conforming_data_frame_passes() {
        let mut o = Oracle::new(CanContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let data = CanFrameDataBuf::from_slice(&[0x11, 0x22, 0x33, 0x44]);

        o.observe(
            t,
            &CanEvent::Frame {
                id: CanId::Standard(0x123),
                rtr: false,
                data: data.clone(),
                acked: true,
            },
        )
        .unwrap();
    }

    #[test]
    fn conforming_remote_frame_passes() {
        let mut o = Oracle::new(CanContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let empty = CanFrameDataBuf::new();

        // Remote frame with zero data length.
        o.observe(
            t,
            &CanEvent::Frame {
                id: CanId::Standard(0x456),
                rtr: true,
                data: empty.clone(),
                acked: true,
            },
        )
        .unwrap();
    }

    #[test]
    fn unacked_frame_breaches_ack() {
        let mut o = Oracle::new(CanContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let data = CanFrameDataBuf::from_slice(&[0x11, 0x22]);

        let v = o
            .observe(
                t,
                &CanEvent::Frame {
                    id: CanId::Standard(0x123),
                    rtr: false,
                    data,
                    acked: false,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "CAN-ACK");
    }

    #[test]
    fn remote_frame_with_data_breaches_rtr_disc() {
        let mut o = Oracle::new(CanContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let data = CanFrameDataBuf::from_slice(&[0x11]);

        let v = o
            .observe(
                t,
                &CanEvent::Frame {
                    id: CanId::Standard(0x456),
                    rtr: true,
                    data,
                    acked: true,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "CAN-RTR-DISC");
    }

    #[test]
    fn extended_id_on_standard_only_bus_breaches_id_field() {
        let cfg = CanConfig {
            extended_ids: false,
            ..CanConfig::default_500k()
        };
        let mut o = Oracle::new(CanContract::new(cfg));
        let t = Nanos::from_micros(10);
        let data = CanFrameDataBuf::new();

        let v = o
            .observe(
                t,
                &CanEvent::Frame {
                    id: CanId::Extended(0x12345678),
                    rtr: false,
                    data,
                    acked: true,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "CAN-ID-FIELD");
    }

    #[test]
    fn crc_error_breaches_crc() {
        let mut o = Oracle::new(CanContract::new(cfg()));
        let t = Nanos::from_micros(10);
        let v = o.observe(t, &CanEvent::CrcError).unwrap_err();
        assert_eq!(v.breach.clause.id, "CAN-CRC");
    }

    #[test]
    fn clause_list_is_complete() {
        let c = CanContract::new(cfg());
        let clauses = c.clauses();
        assert_eq!(clauses.len(), 9);
        let ids: [&str; 9] = [
            "CAN-FRAME-START",
            "CAN-ID-FIELD",
            "CAN-RTR-DISC",
            "CAN-CRC",
            "CAN-ACK",
            "CAN-EOF",
            "CAN-ERROR-FRAME",
            "CAN-BIT-STUFFING",
            "CAN-OVERLOAD",
        ];
        for id in &ids {
            assert!(clauses.iter().any(|cl| cl.id == *id), "missing clause {id}");
        }
    }

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
