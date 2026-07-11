//! The CAN data-link contract: frame field validity and ACK discipline, as
//! an executable state machine judged per-`Frame`-event atomically (no
//! cross-event state — see the crate-level doc comment for why CAN is
//! modeled at frame, not bit, granularity).

use stablehal_contract::{Breach, Clause, Contract, Nanos};

use crate::{CanConfig, CanFrameDataBuf, CanId};

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
}
