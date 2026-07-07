//! CRC-16/MODBUS (poly 0x8005 reflected → 0xA001, init 0xFFFF). AP §6.2.2 /
//! SL §2.5.1.2: the CRC is appended low byte first.

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= u16::from(b);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// True when `frame` (payload + 2 CRC bytes, low first) is CRC-consistent.
pub fn check(frame: &[u8]) -> bool {
    if frame.len() < 3 {
        return false;
    }
    let (payload, tail) = frame.split_at(frame.len() - 2);
    let crc = crc16(payload);
    tail[0] == (crc & 0xFF) as u8 && tail[1] == (crc >> 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector_read_holding() {
        // Classic spec example frame: 01 03 00 00 00 01 → CRC on wire 84 0A.
        let crc = crc16(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x01]);
        assert_eq!((crc & 0xFF) as u8, 0x84);
        assert_eq!((crc >> 8) as u8, 0x0A);
        assert!(check(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A]));
        assert!(!check(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0B]));
    }
}
