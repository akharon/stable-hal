//! RTU timing terms. SL §2.5.1.1: frames are delimited by ≥ t3.5 (3.5
//! character times) of silence; a silent interval > t1.5 inside a frame
//! invalidates it. For baud rates above 19200 the spec fixes t1.5 = 750 µs
//! and t3.5 = 1.75 ms.

use stablehal_contract::{Nanos, TimeBound};
use stablehal_uart::UartConfig;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RtuTiming {
    pub char_time: Nanos,
    pub t15: Nanos,
    pub t35: Nanos,
    /// Upper bound of the response envelope: a slave's response must *start*
    /// within this long after the request's last character. Site-configurable
    /// in real deployments; a fixed contract term here.
    pub response_timeout: Nanos,
}

impl RtuTiming {
    pub fn from_uart(config: &UartConfig, response_timeout: Nanos) -> Self {
        let char_time = config.char_time();
        let (t15, t35) = if config.baud > 19_200 {
            (Nanos::from_micros(750), Nanos::from_micros(1_750)) // SL §2.5.1.1 remark
        } else {
            (char_time.mul_ratio(3, 2), char_time.mul_ratio(7, 2))
        };
        RtuTiming {
            char_time,
            t15,
            t35,
            response_timeout,
        }
    }

    /// The contract envelope for response start, measured from the end of
    /// the request's last character: [t3.5, response_timeout].
    pub fn response_envelope(&self) -> TimeBound {
        TimeBound::new(self.t35, self.response_timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_at_9600_follows_char_time() {
        let t = RtuTiming::from_uart(&UartConfig::rtu_default(9600), Nanos::from_millis(50));
        assert_eq!(t.char_time, Nanos::from_nanos(1_145_833));
        assert_eq!(t.t15, Nanos::from_nanos(1_718_749));
        assert_eq!(t.t35, Nanos::from_nanos(4_010_415));
    }

    #[test]
    fn timing_above_19200_is_fixed() {
        let t = RtuTiming::from_uart(&UartConfig::rtu_default(115_200), Nanos::from_millis(50));
        assert_eq!(t.t15, Nanos::from_micros(750));
        assert_eq!(t.t35, Nanos::from_micros(1_750));
    }
}
