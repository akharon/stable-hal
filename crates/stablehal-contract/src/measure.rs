//! Envelope containment: measurement side-channel of contract judging.
//!
//! A `Contract` already computes measured durations to check timing clauses
//! (that is where `Breach::timing`'s `measured` field comes from). This
//! module lets a contract additionally *report* those measurements — even on
//! the passing path — so a caller can fold them into a per-clause hull
//! (observed `min..=max`). Comparing a hardware-measured hull against a
//! simulator-explored hull is how StableHAL proves the simulator actually
//! exercised the timing region real hardware occupies, closing the gap
//! between "the simulator conforms" and "the hardware conforms" being tested
//! separately.

use core::fmt;

use crate::time::Nanos;

/// One timing observation surfaced during judging.
#[derive(Copy, Clone, Debug)]
pub struct Measurement {
    pub clause_id: &'static str,
    pub value: Nanos,
}

/// The observed range for one clause across a trace: `min..=max`, plus a
/// sample count.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClauseHull {
    pub clause_id: &'static str,
    pub min: Nanos,
    pub max: Nanos,
    pub samples: u64,
}

impl ClauseHull {
    fn observe(&mut self, value: Nanos) {
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
        self.samples += 1;
    }

    /// Does this hull's range sit fully inside `other`'s? An empty hull
    /// (zero samples) is vacuously contained.
    pub fn contained_in(&self, other: &ClauseHull) -> bool {
        self.samples == 0 || (self.min >= other.min && self.max <= other.max)
    }
}

/// Fixed-capacity fold of [`Measurement`]s into per-clause [`ClauseHull`]s.
/// `N` must be at least the number of distinct timing clauses a contract can
/// emit; measurements for a clause beyond capacity are silently dropped
/// (`no_std`, no alloc — capacity is a compile-time contract with the caller).
#[derive(Clone, Debug)]
pub struct EnvelopeAccumulator<const N: usize> {
    hulls: [Option<ClauseHull>; N],
    len: usize,
}

impl<const N: usize> Default for EnvelopeAccumulator<N> {
    fn default() -> Self {
        EnvelopeAccumulator {
            hulls: [None; N],
            len: 0,
        }
    }
}

impl<const N: usize> EnvelopeAccumulator<N> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, m: Measurement) {
        for slot in self.hulls[..self.len].iter_mut().flatten() {
            if slot.clause_id == m.clause_id {
                slot.observe(m.value);
                return;
            }
        }
        if self.len < N {
            self.hulls[self.len] = Some(ClauseHull {
                clause_id: m.clause_id,
                min: m.value,
                max: m.value,
                samples: 1,
            });
            self.len += 1;
        }
        // else: capacity exceeded, measurement dropped.
    }

    pub fn hulls(&self) -> impl Iterator<Item = &ClauseHull> {
        self.hulls[..self.len].iter().filter_map(|h| h.as_ref())
    }

    pub fn hull_for(&self, clause_id: &str) -> Option<&ClauseHull> {
        self.hulls().find(|h| h.clause_id == clause_id)
    }

    /// Fold another accumulator's hulls into this one: union of ranges,
    /// sum of sample counts (hull membership only depends on min/max, so
    /// merging folds through two synthetic observations at the endpoints,
    /// then corrects the sample count to the true sum).
    pub fn merge(&mut self, other: &EnvelopeAccumulator<N>) {
        for h in other.hulls() {
            self.observe(Measurement {
                clause_id: h.clause_id,
                value: h.min,
            });
            self.observe(Measurement {
                clause_id: h.clause_id,
                value: h.max,
            });
            // The two observes above always add exactly 2 to the sample
            // count (new slot: 0 -> 1 -> 2; existing slot: +1 -> +1).
            if let Some(slot) = self.hulls[..self.len]
                .iter_mut()
                .flatten()
                .find(|s| s.clause_id == h.clause_id)
            {
                slot.samples = slot.samples.saturating_sub(2) + h.samples;
            }
        }
    }
}

impl fmt::Display for ClauseHull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}..={} ({} samples)",
            self.clause_id, self.min, self.max, self.samples
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &'static str, ns: u64) -> Measurement {
        Measurement {
            clause_id: id,
            value: Nanos::from_nanos(ns),
        }
    }

    #[test]
    fn folds_min_max_per_clause() {
        let mut acc: EnvelopeAccumulator<4> = EnvelopeAccumulator::new();
        acc.observe(m("A", 100));
        acc.observe(m("A", 50));
        acc.observe(m("A", 200));
        acc.observe(m("B", 9));
        let a = acc.hull_for("A").unwrap();
        assert_eq!(a.min, Nanos::from_nanos(50));
        assert_eq!(a.max, Nanos::from_nanos(200));
        assert_eq!(a.samples, 3);
        let b = acc.hull_for("B").unwrap();
        assert_eq!(b.min, Nanos::from_nanos(9));
        assert_eq!(b.samples, 1);
    }

    #[test]
    fn capacity_drops_excess_clauses_without_panicking() {
        let mut acc: EnvelopeAccumulator<2> = EnvelopeAccumulator::new();
        acc.observe(m("A", 1));
        acc.observe(m("B", 2));
        acc.observe(m("C", 3)); // dropped: capacity exceeded
        assert!(acc.hull_for("A").is_some());
        assert!(acc.hull_for("B").is_some());
        assert!(acc.hull_for("C").is_none());
    }

    #[test]
    fn contained_in_checks_range_inclusion() {
        let narrow = ClauseHull {
            clause_id: "A",
            min: Nanos::from_nanos(10),
            max: Nanos::from_nanos(20),
            samples: 5,
        };
        let wide = ClauseHull {
            clause_id: "A",
            min: Nanos::from_nanos(0),
            max: Nanos::from_nanos(100),
            samples: 5,
        };
        assert!(narrow.contained_in(&wide));
        assert!(!wide.contained_in(&narrow));
    }

    #[test]
    fn empty_hull_is_vacuously_contained() {
        let empty = ClauseHull {
            clause_id: "A",
            min: Nanos::ZERO,
            max: Nanos::ZERO,
            samples: 0,
        };
        let any = ClauseHull {
            clause_id: "A",
            min: Nanos::from_nanos(5),
            max: Nanos::from_nanos(6),
            samples: 1,
        };
        assert!(empty.contained_in(&any));
    }

    #[test]
    fn merge_unions_ranges_and_sums_samples() {
        let mut a: EnvelopeAccumulator<2> = EnvelopeAccumulator::new();
        a.observe(m("A", 10));
        a.observe(m("A", 20));
        let mut b: EnvelopeAccumulator<2> = EnvelopeAccumulator::new();
        b.observe(m("A", 5));
        b.observe(m("A", 30));
        b.observe(m("A", 25));
        a.merge(&b);
        let hull = a.hull_for("A").unwrap();
        assert_eq!(hull.min, Nanos::from_nanos(5));
        assert_eq!(hull.max, Nanos::from_nanos(30));
        assert_eq!(hull.samples, 5);
    }
}
