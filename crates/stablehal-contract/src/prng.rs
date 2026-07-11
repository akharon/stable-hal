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

    /// Boundary-biased envelope sampling: 25% exact min, 25% exact max,
    /// 50% uniform interior. Draw order is fixed so replay determinism holds.
    /// WP4 §4.2 — use behind `SimConfig::boundary_bias` flag.
    pub fn pick_biased(&mut self, bound: TimeBound) -> Nanos {
        // Branch selector: 0..3
        match self.range_inclusive(0, 3) {
            0 => bound.min,
            1 => bound.max,
            _ => Nanos(self.range_inclusive(
                bound.min.as_nanos().saturating_add(1),
                bound.max.as_nanos().saturating_sub(1),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_reference_vector() {
        // Golden vector: the canonical SplitMix64 output for seed 0, as
        // published by the reference implementation (Steele/Lea/Flood 2014,
        // used by the JDK and xoshiro seeders). Pinning the *actual values*
        // (not just self-consistency) is what protects the load-bearing
        // determinism claim: any change to the constants or mixing steps —
        // which would silently invalidate every recorded trace hash across
        // platforms — fails here first.
        let expected: [u64; 5] = [
            0xe220a8397b1dcdaf,
            0x6e789e6aa1b965f4,
            0x06c45d188009454f,
            0xf88bb8a8724c81ec,
            0x1b39896a51a8749b,
        ];
        let mut r = SplitMix64::new(0);
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(r.next_u64(), *want, "SplitMix64(0) output {i} drifted");
        }

        // Same seed is reproducible; distinct seeds diverge immediately.
        assert_eq!(SplitMix64::new(0).next_u64(), SplitMix64::new(0).next_u64());
        assert_ne!(SplitMix64::new(0).next_u64(), SplitMix64::new(1).next_u64());
    }

    #[test]
    fn pick_stays_inside_envelope() {
        let mut r = SplitMix64::new(42);
        let b = TimeBound::new(Nanos(100), Nanos(200));
        for _ in 0..10_000 {
            assert!(b.contains(r.pick(b)));
        }
    }

    #[test]
    fn pick_biased_hits_boundaries_and_interior() {
        let mut r = SplitMix64::new(42);
        let b = TimeBound::new(Nanos(100_000), Nanos(200_000));
        let mut saw_min = false;
        let mut saw_max = false;
        let mut saw_interior = false;
        for _ in 0..10_000 {
            let v = r.pick_biased(b);
            assert!(b.contains(v), "biased pick outside envelope");
            if v == b.min {
                saw_min = true;
            } else if v == b.max {
                saw_max = true;
            } else {
                saw_interior = true;
            }
        }
        assert!(saw_min, "never picked the min boundary");
        assert!(saw_max, "never picked the max boundary");
        assert!(saw_interior, "never picked an interior value");
    }
}
