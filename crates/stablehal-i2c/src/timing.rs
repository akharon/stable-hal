//! I2C timing envelopes (UM10204 rev 7, Table 10) plus the deployment-level
//! clock-stretch cap (the spec itself allows unbounded stretching; a real
//! system must bound it — default follows the SMBus 3.x convention).

use stablehal_contract::{Nanos, TimeBound};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum I2cSpeed {
    Standard100k,
    Fast400k,
}

#[derive(Copy, Clone, Debug)]
pub struct I2cTiming {
    pub speed: I2cSpeed,
    /// SCL period envelope actually driven by the master. UM10204 §6: masters
    /// may run slower than nominal; never faster than the mode maximum.
    pub scl: TimeBound,
    /// Bus-free time between Stop and the next Start (tBUF). UM10204 Table 10.
    pub bus_free_min: Nanos,
    /// Contract cap on cumulative clock stretching per byte, by the slave.
    pub stretch_max: Nanos,
}

impl I2cTiming {
    /// Standard-mode (100 kHz) envelope: period 10..=100us (loosely bounded
    /// below the 100kHz nominal; UM10204 only mandates a *maximum* rate, so
    /// the practical floor here is a deployment choice, not a spec minimum),
    /// tBUF >= 4.7us.
    pub const fn standard() -> Self {
        I2cTiming {
            speed: I2cSpeed::Standard100k,
            scl: TimeBound::new(Nanos::from_micros(10), Nanos::from_micros(100)),
            bus_free_min: Nanos::from_nanos(4_700),
            stretch_max: Nanos::from_millis(25),
        }
    }

    /// Fast-mode (400 kHz) envelope: period 2.5..=25us, tBUF >= 1.3us.
    pub const fn fast() -> Self {
        I2cTiming {
            speed: I2cSpeed::Fast400k,
            scl: TimeBound::new(Nanos::from_nanos(2_500), Nanos::from_micros(25)),
            bus_free_min: Nanos::from_nanos(1_300),
            stretch_max: Nanos::from_millis(25),
        }
    }

    /// Nominal time for 9 SCL cycles (8 data bits + ACK) at a given period.
    pub const fn byte_time(scl_period: Nanos) -> Nanos {
        scl_period.mul_ratio(9, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_byte_time_is_nine_periods() {
        let t = I2cTiming::standard();
        assert_eq!(
            I2cTiming::byte_time(t.scl.max),
            Nanos::from_micros(100).mul_ratio(9, 1)
        );
    }

    #[test]
    fn fast_mode_has_tighter_envelope_than_standard() {
        let std = I2cTiming::standard();
        let fast = I2cTiming::fast();
        assert!(fast.scl.max.as_nanos() < std.scl.max.as_nanos());
        assert!(fast.bus_free_min.as_nanos() < std.bus_free_min.as_nanos());
    }
}
