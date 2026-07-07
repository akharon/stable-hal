//! Virtual time. All StableHAL components — contracts, simulator, drivers —
//! speak `Nanos`; wall-clock time never appears anywhere in the system.

use core::fmt;
use core::ops::{Add, AddAssign, Sub};

/// A point in (or span of) virtual time, in nanoseconds.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Nanos(pub u64);

impl Nanos {
    pub const ZERO: Nanos = Nanos(0);
    pub const MAX: Nanos = Nanos(u64::MAX);

    pub const fn from_nanos(n: u64) -> Self {
        Nanos(n)
    }
    pub const fn from_micros(us: u64) -> Self {
        Nanos(us * 1_000)
    }
    pub const fn from_millis(ms: u64) -> Self {
        Nanos(ms * 1_000_000)
    }
    pub const fn from_secs(s: u64) -> Self {
        Nanos(s * 1_000_000_000)
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Nanos(self.0.saturating_sub(rhs.0))
    }
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Nanos(self.0.saturating_add(rhs.0))
    }
    /// Integer multiply by a rational factor, e.g. `char_time.mul_ratio(7, 2)`
    /// for 3.5×. Truncates toward zero; contracts must use this consistently
    /// so simulator and on-target monitor compute identical bounds.
    pub const fn mul_ratio(self, num: u64, den: u64) -> Self {
        Nanos(self.0 * num / den)
    }
}

impl Add for Nanos {
    type Output = Nanos;
    fn add(self, rhs: Self) -> Self {
        Nanos(self.0 + rhs.0)
    }
}
impl AddAssign for Nanos {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}
impl Sub for Nanos {
    type Output = Nanos;
    fn sub(self, rhs: Self) -> Self {
        Nanos(self.0 - rhs.0)
    }
}

impl fmt::Debug for Nanos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ns", self.0)
    }
}

impl fmt::Display for Nanos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0;
        if n == u64::MAX {
            write!(f, "∞")
        } else if n >= 1_000_000_000 && n % 1_000_000 == 0 {
            write!(f, "{}.{:03}s", n / 1_000_000_000, (n / 1_000_000) % 1_000)
        } else if n >= 1_000_000 {
            write!(f, "{}.{:03}ms", n / 1_000_000, (n / 1_000) % 1_000)
        } else if n >= 1_000 {
            write!(f, "{}.{:03}us", n / 1_000, n % 1_000)
        } else {
            write!(f, "{}ns", n)
        }
    }
}

/// An inclusive envelope `min..=max` of virtual time. Timing obligations in
/// contracts are always envelopes: the allowed nondeterminism is the width.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct TimeBound {
    pub min: Nanos,
    pub max: Nanos,
}

impl TimeBound {
    pub const fn new(min: Nanos, max: Nanos) -> Self {
        TimeBound { min, max }
    }
    /// A zero-width envelope: exactly `t`.
    pub const fn exact(t: Nanos) -> Self {
        TimeBound { min: t, max: t }
    }
    /// `at_least(t)`: no upper obligation.
    pub const fn at_least(t: Nanos) -> Self {
        TimeBound {
            min: t,
            max: Nanos::MAX,
        }
    }
    pub const fn contains(&self, d: Nanos) -> bool {
        self.min.0 <= d.0 && d.0 <= self.max.0
    }
    pub const fn width(&self) -> Nanos {
        Nanos(self.max.0.saturating_sub(self.min.0))
    }
}

impl fmt::Debug for TimeBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..={}", self.min, self.max)
    }
}
impl fmt::Display for TimeBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..={}", self.min, self.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_contains_is_inclusive() {
        let b = TimeBound::new(Nanos(10), Nanos(20));
        assert!(b.contains(Nanos(10)));
        assert!(b.contains(Nanos(20)));
        assert!(!b.contains(Nanos(9)));
        assert!(!b.contains(Nanos(21)));
    }

    #[test]
    fn mul_ratio_truncates_deterministically() {
        // 3.5 × 1_145_833ns (11-bit char @ 9600 baud) — the MODBUS t3.5 case.
        assert_eq!(Nanos(1_145_833).mul_ratio(7, 2), Nanos(4_010_415));
    }
}
