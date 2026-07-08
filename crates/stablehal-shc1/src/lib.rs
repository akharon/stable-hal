//! SHC1 capture stream format (WP3 §3.1): binary, little-endian, designed so
//! a PIO/ISR can emit it with no buffering cleverness and the host can
//! resynchronize after corruption. This crate hand-rolls the codec (no
//! third-party deps) and maps records to/from `RtuEvent` exactly as
//! `stablehal_conformance::runner::rtu_observer` does, so the same
//! `RtuContract` oracle judges both simulated and captured traces.
//!
//! ```text
//! Header (16 bytes, once per stream):
//!   magic       u32  = 0x31434853        ("SHC1")
//!   contract    u16  = 0x0001            (rtu v0.1; registry in design-spec)
//!   flags       u16  = bit0: self_report (tester timestamps its own TX)
//!                      bit1: observer    (independent bus observer)
//!   baud        u32
//!   uart_cfg    u8   = bits[0..4] data_bits, bit4 parity_en, bit5 parity_odd,
//!                      bits[6..8] stop_bits
//!   reserved    u8[3] = 0
//!
//! Record (12 bytes, repeating):
//!   sync        u8   = 0xA5
//!   kind        u8   = 0: TxStart, 1: TxEnd, 2: DeliveredOk, 3: DeliveredErr,
//!                      0xFF: EndOfObservation
//!   line        u8   = 0: master->slave, 1: slave->master
//!   payload     u8   = byte value (kind 0..2), UartError discriminant (kind 3)
//!   t_ns        u64  = virtual/monotonic nanoseconds since stream start
//! ```

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

use stablehal_contract::{Nanos, TraceEvent};
use stablehal_modbus::contract::{Dir, RtuEvent};
use stablehal_uart::{LineEvent, Parity, UartConfig, UartError};

pub const MAGIC: u32 = 0x3143_4853;
pub const CONTRACT_RTU_V0_1: u16 = 0x0001;
pub const FLAG_SELF_REPORT: u16 = 1 << 0;
pub const FLAG_OBSERVER: u16 = 1 << 1;

pub const HEADER_LEN: usize = 16;
pub const RECORD_LEN: usize = 12;
pub const SYNC: u8 = 0xA5;

pub const KIND_TX_START: u8 = 0;
pub const KIND_TX_END: u8 = 1;
pub const KIND_DELIVERED_OK: u8 = 2;
pub const KIND_DELIVERED_ERR: u8 = 3;
pub const KIND_END_OF_OBSERVATION: u8 = 0xFF;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub contract: u16,
    pub flags: u16,
    pub baud: u32,
    pub uart_cfg: UartConfig,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[4..6].copy_from_slice(&self.contract.to_le_bytes());
        buf[6..8].copy_from_slice(&self.flags.to_le_bytes());
        buf[8..12].copy_from_slice(&self.baud.to_le_bytes());
        buf[12] = encode_uart_cfg(&self.uart_cfg);
        // buf[13..16] reserved = 0
        buf
    }

    pub fn decode(buf: &[u8]) -> Result<Self, ShcError> {
        if buf.len() < HEADER_LEN {
            return Err(ShcError::Truncated);
        }
        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != MAGIC {
            return Err(ShcError::BadMagic);
        }
        let contract = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        let flags = u16::from_le_bytes(buf[6..8].try_into().unwrap());
        let baud = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        let uart_cfg = decode_uart_cfg(buf[12], baud);
        Ok(Header {
            contract,
            flags,
            baud,
            uart_cfg,
        })
    }
}

fn encode_uart_cfg(cfg: &UartConfig) -> u8 {
    let mut b = cfg.data_bits & 0x0F;
    let (parity_en, parity_odd) = match cfg.parity {
        Parity::None => (0u8, 0u8),
        Parity::Even => (1, 0),
        Parity::Odd => (1, 1),
    };
    b |= parity_en << 4;
    b |= parity_odd << 5;
    b |= (cfg.stop_bits & 0x03) << 6;
    b
}

fn decode_uart_cfg(b: u8, baud: u32) -> UartConfig {
    let data_bits = b & 0x0F;
    let parity_en = (b >> 4) & 1;
    let parity_odd = (b >> 5) & 1;
    let stop_bits = (b >> 6) & 0x03;
    let parity = match (parity_en, parity_odd) {
        (0, _) => Parity::None,
        (_, 0) => Parity::Even,
        _ => Parity::Odd,
    };
    UartConfig {
        baud,
        data_bits,
        parity,
        stop_bits,
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: u8,
    pub line: u8,
    pub payload: u8,
    pub t_ns: u64,
}

impl Record {
    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut buf = [0u8; RECORD_LEN];
        buf[0] = SYNC;
        buf[1] = self.kind;
        buf[2] = self.line;
        buf[3] = self.payload;
        buf[4..12].copy_from_slice(&self.t_ns.to_le_bytes());
        buf
    }

    fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < RECORD_LEN || buf[0] != SYNC {
            return None;
        }
        let kind = buf[1];
        if !(kind <= KIND_DELIVERED_ERR || kind == KIND_END_OF_OBSERVATION) {
            return None;
        }
        let t_ns = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        Some(Record {
            kind,
            line: buf[2],
            payload: buf[3],
            t_ns,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShcError {
    Truncated,
    BadMagic,
}

fn uart_error_to_discriminant(e: UartError) -> u8 {
    match e {
        UartError::Framing => 0,
        UartError::Parity => 1,
        UartError::Noise => 2,
        UartError::Overrun => 3,
    }
}
fn discriminant_to_uart_error(d: u8) -> UartError {
    match d {
        0 => UartError::Framing,
        1 => UartError::Parity,
        2 => UartError::Noise,
        _ => UartError::Overrun,
    }
}

/// Serialize a simulator wire trace (`(line, LineEvent)` events) into an
/// SHC1 byte stream.
pub fn write_stream(header: &Header, trace: &[TraceEvent<(u8, LineEvent)>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + trace.len() * RECORD_LEN);
    out.extend_from_slice(&header.encode());
    for ev in trace {
        let t_ns = ev.at.as_nanos();
        let (line, levent) = ev.event;
        let record = match levent {
            LineEvent::TxStart { byte } => Record {
                kind: KIND_TX_START,
                line,
                payload: byte,
                t_ns,
            },
            LineEvent::TxEnd { byte } => Record {
                kind: KIND_TX_END,
                line,
                payload: byte,
                t_ns,
            },
            LineEvent::Delivered { byte } => Record {
                kind: KIND_DELIVERED_OK,
                line,
                payload: byte,
                t_ns,
            },
            LineEvent::DeliveredCorrupt { error } => Record {
                kind: KIND_DELIVERED_ERR,
                line,
                payload: uart_error_to_discriminant(error),
                t_ns,
            },
        };
        out.extend_from_slice(&record.encode());
    }
    out
}

/// Append an `EndOfObservation` record at `t_ns`.
pub fn append_end_of_observation(out: &mut Vec<u8>, t_ns: u64) {
    out.extend_from_slice(
        &Record {
            kind: KIND_END_OF_OBSERVATION,
            line: 0,
            payload: 0,
            t_ns,
        }
        .encode(),
    );
}

#[derive(Debug, Default)]
pub struct ParseReport {
    /// Records successfully parsed and mapped.
    pub records: usize,
    /// Bytes skipped while resynchronizing after a parse failure.
    pub capture_gaps: usize,
}

/// Parse an SHC1 stream into `(header, RtuEvent trace, report)`. On a
/// corrupt record, scans forward to the next `0xA5` whose record passes the
/// resync predicate (valid kind, monotonic `t_ns`), counting skipped bytes
/// into `capture_gaps` — WP3 §3.1 resync rule.
pub fn parse_stream(
    bytes: &[u8],
) -> Result<(Header, Vec<TraceEvent<RtuEvent>>, ParseReport), ShcError> {
    let header = Header::decode(bytes)?;
    let mut events = Vec::new();
    let mut report = ParseReport::default();
    let mut pos = HEADER_LEN;
    let mut last_t: u64 = 0;

    while pos < bytes.len() {
        match Record::decode(&bytes[pos..]) {
            Some(rec) if rec.t_ns >= last_t => {
                last_t = rec.t_ns;
                report.records += 1;
                let dir = if rec.line == 0 {
                    Dir::MasterToSlave
                } else {
                    Dir::SlaveToMaster
                };
                let at = Nanos::from_nanos(rec.t_ns);
                match rec.kind {
                    KIND_TX_START => {
                        events.push(TraceEvent {
                            at,
                            event: RtuEvent::CharStart {
                                dir,
                                byte: rec.payload,
                            },
                        });
                    }
                    KIND_TX_END => {
                        events.push(TraceEvent {
                            at,
                            event: RtuEvent::CharEnd {
                                dir,
                                byte: rec.payload,
                            },
                        });
                    }
                    KIND_DELIVERED_OK | KIND_DELIVERED_ERR => {
                        // Environment outcomes carry no protocol judgement
                        // (mirrors rtu_observer's mapping: ignored for
                        // RtuContract purposes).
                        let _ = discriminant_to_uart_error(rec.payload);
                    }
                    KIND_END_OF_OBSERVATION => {
                        events.push(TraceEvent {
                            at,
                            event: RtuEvent::EndOfObservation,
                        });
                    }
                    _ => unreachable!("Record::decode filters kind"),
                }
                pos += RECORD_LEN;
            }
            _ => {
                // Resync: scan forward for the next plausible sync byte.
                let mut skip = 1;
                while pos + skip < bytes.len() && bytes[pos + skip] != SYNC {
                    skip += 1;
                }
                report.capture_gaps += skip;
                pos += skip;
            }
        }
    }

    Ok((header, events, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let h = Header {
            contract: CONTRACT_RTU_V0_1,
            flags: FLAG_OBSERVER,
            baud: 9600,
            uart_cfg: UartConfig::rtu_default(9600),
        };
        let encoded = h.encode();
        let decoded = Header::decode(&encoded).unwrap();
        assert_eq!(decoded, h);
    }

    #[test]
    fn record_round_trips() {
        let r = Record {
            kind: KIND_TX_START,
            line: 1,
            payload: 0x42,
            t_ns: 123_456_789,
        };
        let encoded = r.encode();
        let decoded = Record::decode(&encoded).unwrap();
        assert_eq!(decoded, r);
    }

    #[test]
    fn resync_skips_corrupt_bytes_and_counts_gaps() {
        let header = Header {
            contract: CONTRACT_RTU_V0_1,
            flags: 0,
            baud: 9600,
            uart_cfg: UartConfig::rtu_default(9600),
        };
        let mut stream = header.encode().to_vec();
        stream.extend_from_slice(
            &Record {
                kind: KIND_TX_START,
                line: 0,
                payload: 1,
                t_ns: 0,
            }
            .encode(),
        );
        // Corrupt garbage injected between two valid records.
        stream.extend_from_slice(&[0x00, 0x11, 0x22]);
        stream.extend_from_slice(
            &Record {
                kind: KIND_TX_END,
                line: 0,
                payload: 1,
                t_ns: 1000,
            }
            .encode(),
        );
        let (_h, events, report) = parse_stream(&stream).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(report.records, 2);
        assert_eq!(report.capture_gaps, 3);
    }
}
