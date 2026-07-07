//! ADU/PDU construction and parsing for the v0.1 subset
//! (FC 0x03 / 0x06 / 0x10 + exceptions). AP §6.

use core::fmt;

use crate::crc;

/// SL §2.5.1: maximum RTU ADU size is 256 bytes.
pub const MAX_ADU: usize = 256;

pub const FC_READ_HOLDING: u8 = 0x03;
pub const FC_WRITE_SINGLE: u8 = 0x06;
pub const FC_WRITE_MULTIPLE: u8 = 0x10;

/// A frame buffer sized for the worst case. `no_std`, no allocation.
#[derive(Clone)]
pub struct Adu {
    buf: [u8; MAX_ADU],
    len: usize,
}

impl Adu {
    pub const fn new() -> Self {
        Adu {
            buf: [0; MAX_ADU],
            len: 0,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut adu = Adu::new();
        for &b in bytes.iter().take(MAX_ADU) {
            adu.push(b);
        }
        adu
    }

    /// Appends a byte; silently saturates at MAX_ADU (a real line would have
    /// framed off long before — the contract flags oversized frames anyway).
    pub fn push(&mut self, byte: u8) {
        if self.len < MAX_ADU {
            self.buf[self.len] = byte;
            self.len += 1;
        }
    }

    pub fn push_u16(&mut self, v: u16) {
        self.push((v >> 8) as u8);
        self.push((v & 0xFF) as u8);
    }

    pub fn append_crc(&mut self) {
        let c = crc::crc16(self.bytes());
        self.push((c & 0xFF) as u8);
        self.push((c >> 8) as u8);
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn clear(&mut self) {
        self.len = 0;
    }
    /// Mutable access for deliberate corruption (buggy slaves, test probes).
    pub fn byte_mut(&mut self, i: usize) -> &mut u8 {
        &mut self.buf[i]
    }
}

impl Default for Adu {
    fn default() -> Self {
        Adu::new()
    }
}

impl PartialEq for Adu {
    fn eq(&self, other: &Self) -> bool {
        self.bytes() == other.bytes()
    }
}
impl Eq for Adu {}

impl fmt::Debug for Adu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Adu[")?;
        for (i, b) in self.bytes().iter().enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            write!(f, "{:02X}", b)?;
        }
        write!(f, "]")
    }
}

/// AP §7 exception codes (subset relevant to FC 03/06/10).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExceptionCode {
    IllegalFunction = 0x01,
    IllegalDataAddress = 0x02,
    IllegalDataValue = 0x03,
    ServerDeviceFailure = 0x04,
}

impl ExceptionCode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(ExceptionCode::IllegalFunction),
            0x02 => Some(ExceptionCode::IllegalDataAddress),
            0x03 => Some(ExceptionCode::IllegalDataValue),
            0x04 => Some(ExceptionCode::ServerDeviceFailure),
            _ => None,
        }
    }
}

/// Requests in the v0.1 subset.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    ReadHoldingRegisters { addr: u16, count: u16 },
    WriteSingleRegister { addr: u16, value: u16 },
    WriteMultipleRegisters { addr: u16, values: &'a [u16] },
}

impl Request<'_> {
    pub fn function_code(&self) -> u8 {
        match self {
            Request::ReadHoldingRegisters { .. } => FC_READ_HOLDING,
            Request::WriteSingleRegister { .. } => FC_WRITE_SINGLE,
            Request::WriteMultipleRegisters { .. } => FC_WRITE_MULTIPLE,
        }
    }
}

/// Build a complete request ADU (unit + PDU + CRC). AP §6.3/6.6/6.12.
pub fn build_request(unit: u8, req: &Request<'_>, out: &mut Adu) {
    out.clear();
    out.push(unit);
    out.push(req.function_code());
    match *req {
        Request::ReadHoldingRegisters { addr, count } => {
            out.push_u16(addr);
            out.push_u16(count);
        }
        Request::WriteSingleRegister { addr, value } => {
            out.push_u16(addr);
            out.push_u16(value);
        }
        Request::WriteMultipleRegisters { addr, values } => {
            out.push_u16(addr);
            out.push_u16(values.len() as u16);
            out.push((values.len() * 2) as u8);
            for &v in values {
                out.push_u16(v);
            }
        }
    }
    out.append_crc();
}

pub fn build_read_response(unit: u8, values: &[u16], out: &mut Adu) {
    out.clear();
    out.push(unit);
    out.push(FC_READ_HOLDING);
    out.push((values.len() * 2) as u8);
    for &v in values {
        out.push_u16(v);
    }
    out.append_crc();
}

/// FC 06 response echoes the request. AP §6.6.
pub fn build_write_single_response(unit: u8, addr: u16, value: u16, out: &mut Adu) {
    out.clear();
    out.push(unit);
    out.push(FC_WRITE_SINGLE);
    out.push_u16(addr);
    out.push_u16(value);
    out.append_crc();
}

/// FC 16 response echoes address + quantity. AP §6.12.
pub fn build_write_multiple_response(unit: u8, addr: u16, count: u16, out: &mut Adu) {
    out.clear();
    out.push(unit);
    out.push(FC_WRITE_MULTIPLE);
    out.push_u16(addr);
    out.push_u16(count);
    out.append_crc();
}

/// AP §7: exception response = unit, fc|0x80, code.
pub fn build_exception(unit: u8, fc: u8, code: ExceptionCode, out: &mut Adu) {
    out.clear();
    out.push(unit);
    out.push(fc | 0x80);
    out.push(code as u8);
    out.append_crc();
}

fn be16(hi: u8, lo: u8) -> u16 {
    (u16::from(hi) << 8) | u16::from(lo)
}

/// A request as seen by a slave, after CRC and length validation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParsedRequest<'a> {
    ReadHolding { addr: u16, count: u16 },
    WriteSingle { addr: u16, value: u16 },
    /// `data` holds `count` big-endian u16 values.
    WriteMultiple { addr: u16, count: u16, data: &'a [u8] },
    /// Structurally sound frame with a function code outside the subset —
    /// the slave must answer exception 01. AP §7.
    UnknownFunction { fc: u8 },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FrameFault {
    TooShort,
    BadCrc,
    /// Length inconsistent with the function code — indistinguishable from
    /// line noise; the slave discards silently. SL §2.5.1.1.
    Malformed,
}

/// Parse a received request ADU. Returns the addressed unit and the request.
pub fn parse_request(frame: &[u8]) -> Result<(u8, ParsedRequest<'_>), FrameFault> {
    if frame.len() < 4 {
        return Err(FrameFault::TooShort);
    }
    if !crc::check(frame) {
        return Err(FrameFault::BadCrc);
    }
    let unit = frame[0];
    let fc = frame[1];
    let body = &frame[2..frame.len() - 2];
    let req = match fc {
        FC_READ_HOLDING => {
            if body.len() != 4 {
                return Err(FrameFault::Malformed);
            }
            ParsedRequest::ReadHolding {
                addr: be16(body[0], body[1]),
                count: be16(body[2], body[3]),
            }
        }
        FC_WRITE_SINGLE => {
            if body.len() != 4 {
                return Err(FrameFault::Malformed);
            }
            ParsedRequest::WriteSingle {
                addr: be16(body[0], body[1]),
                value: be16(body[2], body[3]),
            }
        }
        FC_WRITE_MULTIPLE => {
            if body.len() < 5 {
                return Err(FrameFault::Malformed);
            }
            let addr = be16(body[0], body[1]);
            let count = be16(body[2], body[3]);
            let byte_count = body[4] as usize;
            let data = &body[5..];
            if data.len() != byte_count || byte_count != count as usize * 2 {
                return Err(FrameFault::Malformed);
            }
            ParsedRequest::WriteMultiple { addr, count, data }
        }
        _ => ParsedRequest::UnknownFunction { fc },
    };
    Ok((unit, req))
}

/// A response as seen by the master, validated against its pending request.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParsedResponse<'a> {
    /// FC 03: `data` holds the register bytes (big-endian pairs).
    ReadHolding { data: &'a [u8] },
    WriteSingle { addr: u16, value: u16 },
    WriteMultiple { addr: u16, count: u16 },
    Exception { code: ExceptionCode },
}

impl ParsedResponse<'_> {
    pub fn register(&self, i: usize) -> Option<u16> {
        match self {
            ParsedResponse::ReadHolding { data } => {
                if i * 2 + 1 < data.len() {
                    Some(be16(data[i * 2], data[i * 2 + 1]))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    pub fn register_count(&self) -> usize {
        match self {
            ParsedResponse::ReadHolding { data } => data.len() / 2,
            _ => 0,
        }
    }
}

/// Validate a response frame against the request context (expected unit and
/// function code).
pub fn parse_response<'a>(
    expect_unit: u8,
    expect_fc: u8,
    frame: &'a [u8],
) -> Result<ParsedResponse<'a>, FrameFault> {
    if frame.len() < 4 {
        return Err(FrameFault::TooShort);
    }
    if !crc::check(frame) {
        return Err(FrameFault::BadCrc);
    }
    if frame[0] != expect_unit {
        return Err(FrameFault::Malformed);
    }
    let fc = frame[1];
    let body = &frame[2..frame.len() - 2];
    if fc == expect_fc | 0x80 {
        if body.len() != 1 {
            return Err(FrameFault::Malformed);
        }
        let code = ExceptionCode::from_u8(body[0]).ok_or(FrameFault::Malformed)?;
        return Ok(ParsedResponse::Exception { code });
    }
    if fc != expect_fc {
        return Err(FrameFault::Malformed);
    }
    match fc {
        FC_READ_HOLDING => {
            if body.is_empty() || body.len() != 1 + body[0] as usize || body[0] % 2 != 0 {
                return Err(FrameFault::Malformed);
            }
            Ok(ParsedResponse::ReadHolding { data: &body[1..] })
        }
        FC_WRITE_SINGLE => {
            if body.len() != 4 {
                return Err(FrameFault::Malformed);
            }
            Ok(ParsedResponse::WriteSingle {
                addr: be16(body[0], body[1]),
                value: be16(body[2], body[3]),
            })
        }
        FC_WRITE_MULTIPLE => {
            if body.len() != 4 {
                return Err(FrameFault::Malformed);
            }
            Ok(ParsedResponse::WriteMultiple {
                addr: be16(body[0], body[1]),
                count: be16(body[2], body[3]),
            })
        }
        _ => Err(FrameFault::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_read() {
        let mut adu = Adu::new();
        build_request(
            0x11,
            &Request::ReadHoldingRegisters { addr: 5, count: 3 },
            &mut adu,
        );
        let (unit, req) = parse_request(adu.bytes()).unwrap();
        assert_eq!(unit, 0x11);
        assert_eq!(req, ParsedRequest::ReadHolding { addr: 5, count: 3 });
    }

    #[test]
    fn request_roundtrip_write_multiple() {
        let mut adu = Adu::new();
        build_request(
            0x11,
            &Request::WriteMultipleRegisters {
                addr: 10,
                values: &[0xAAAA, 0x5555],
            },
            &mut adu,
        );
        let (unit, req) = parse_request(adu.bytes()).unwrap();
        assert_eq!(unit, 0x11);
        match req {
            ParsedRequest::WriteMultiple { addr, count, data } => {
                assert_eq!((addr, count), (10, 2));
                assert_eq!(data, &[0xAA, 0xAA, 0x55, 0x55]);
            }
            other => panic!("wrong parse: {:?}", other),
        }
    }

    #[test]
    fn response_roundtrip_and_exception() {
        let mut adu = Adu::new();
        build_read_response(0x11, &[1, 2, 3], &mut adu);
        let r = parse_response(0x11, FC_READ_HOLDING, adu.bytes()).unwrap();
        assert_eq!(r.register_count(), 3);
        assert_eq!(r.register(2), Some(3));

        build_exception(0x11, FC_READ_HOLDING, ExceptionCode::IllegalDataAddress, &mut adu);
        let r = parse_response(0x11, FC_READ_HOLDING, adu.bytes()).unwrap();
        assert_eq!(
            r,
            ParsedResponse::Exception {
                code: ExceptionCode::IllegalDataAddress
            }
        );
    }

    #[test]
    fn corrupt_crc_is_rejected() {
        let mut adu = Adu::new();
        build_request(1, &Request::ReadHoldingRegisters { addr: 0, count: 1 }, &mut adu);
        let last = adu.len() - 1;
        *adu.byte_mut(last) ^= 0xFF;
        assert_eq!(parse_request(adu.bytes()), Err(FrameFault::BadCrc));
    }
}
