//! Deterministic event scheduler: a `(at, seq)`-ordered priority queue plus
//! the single seeded PRNG stream, generic over the job payload type.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use stablehal_contract::{Nanos, SplitMix64};

struct Scheduled<J> {
    at: Nanos,
    seq: u64,
    job: J,
}

impl<J> PartialEq for Scheduled<J> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.seq == other.seq
    }
}
impl<J> Eq for Scheduled<J> {}
impl<J> PartialOrd for Scheduled<J> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<J> Ord for Scheduled<J> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.at, self.seq).cmp(&(other.at, other.seq))
    }
}

pub struct Scheduler<J> {
    now: Nanos,
    seq: u64,
    queue: BinaryHeap<Reverse<Scheduled<J>>>,
    prng: SplitMix64,
}

impl<J> Scheduler<J> {
    pub fn new(seed: u64) -> Self {
        Scheduler {
            now: Nanos::ZERO,
            seq: 0,
            queue: BinaryHeap::new(),
            prng: SplitMix64::new(seed),
        }
    }

    pub fn now(&self) -> Nanos {
        self.now
    }

    /// Schedule `job` at `at`, clamped to `>= now` (virtual time never runs
    /// backwards). Ties at the same timestamp are broken by insertion order.
    pub fn schedule(&mut self, at: Nanos, job: J) {
        let at = at.max(self.now);
        let seq = self.seq;
        self.seq += 1;
        self.queue.push(Reverse(Scheduled { at, seq, job }));
    }

    /// Pop the next job due at or before `deadline`, advancing `now` to its
    /// timestamp. Returns `None` once the queue is empty or the next job is
    /// past the deadline (in which case it stays queued).
    pub fn pop_due(&mut self, deadline: Nanos) -> Option<(Nanos, J)> {
        if self.queue.peek()?.0.at > deadline {
            return None;
        }
        let Reverse(next) = self.queue.pop()?;
        debug_assert!(next.at >= self.now, "virtual time must not go backwards");
        self.now = next.at;
        Some((next.at, next.job))
    }

    pub fn prng(&mut self) -> &mut SplitMix64 {
        &mut self.prng
    }
}
