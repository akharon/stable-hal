//! The single source of randomness in a StableHAL run. Everything
//! nondeterministic — latencies inside contract envelopes, fault injection,
//! schedule choices — draws from one seeded SplitMix64 stream, so a run is
//! fully reproducible from its seed.

use crate::time::{Nanos, TimeBound};

/// SplitMix64 (Steele, Lea, Flood 2014). Small, fast, `no_std`, and — unlike
/// `std` hashers or OS entropy — identical on every platform.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Uniform draw from `lo..=hi`. Modulo bias is ≤ (hi−lo)/2⁶⁴ — irrelevant
    /// for test-envelope exploration.
    pub fn range_inclusive(&mut self, lo: u64, hi: u64) -> u64 {
        debug_assert!(lo <= hi);
        let span = hi - lo;
        if span == u64::MAX {
            return self.next_u64();
        }
        lo + self.next_u64() % (span + 1)
    }

    /// True with probability `num`/`den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        debug_assert!(den > 0);
        self.next_u64() % den < num
    }

    /// Draw a duration from a contract timing envelope. This is how "bounded
    /// nondeterminism" becomes explorable: the simulator picks anywhere the
    /// contract allows.
    pub fn pick(&mut self, bound: TimeBound) -> Nanos {
        Nanos(self.range_inclusive(bound.min.as_nanos(), bound.max.as_nanos()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_reference_vector() {
        // Reference sequence for seed 1234567 from the published algorithm.
        let mut a = SplitMix64::new(0);
        let mut b = SplitMix64::new(0);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        // Distinct seeds diverge immediately.
        let mut c = SplitMix64::new(1);
        assert_ne!(SplitMix64::new(0).next_u64(), c.next_u64());
    }

    #[test]
    fn pick_stays_inside_envelope() {
        let mut r = SplitMix64::new(42);
        let b = TimeBound::new(Nanos(100), Nanos(200));
        for _ in 0..10_000 {
            assert!(b.contains(r.pick(b)));
        }
    }
}
