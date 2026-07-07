//! Canonical traces and the deterministic trace hash.
//!
//! Every run — simulated or captured on hardware — reduces to a sequence of
//! `TraceEvent`s. The FNV-1a hash over their canonical rendering is the
//! fingerprint used to prove seed-replay determinism: same seed ⇒ same hash,
//! byte for byte, across runs and platforms.

use core::fmt::{self, Write as _};

use crate::time::Nanos;

#[derive(Clone, Debug, PartialEq)]
pub struct TraceEvent<E> {
    pub at: Nanos,
    pub event: E,
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001b3;

/// FNV-1a accumulator exposed as a `fmt::Write` sink, so anything with a
/// `Debug` rendering can be hashed without allocation.
pub struct HashWriter {
    state: u64,
}

impl HashWriter {
    pub fn new() -> Self {
        HashWriter { state: FNV_OFFSET }
    }
    pub fn finish(&self) -> u64 {
        self.state
    }
}

impl Default for HashWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Write for HashWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.as_bytes() {
            self.state ^= u64::from(*b);
            self.state = self.state.wrapping_mul(FNV_PRIME);
        }
        Ok(())
    }
}

/// Deterministic fingerprint of a trace.
pub fn trace_hash<'a, E: fmt::Debug + 'a>(
    events: impl IntoIterator<Item = &'a TraceEvent<E>>,
) -> u64 {
    let mut w = HashWriter::new();
    for e in events {
        // Canonical rendering: `<nanos>:<debug>;`. All event types in
        // StableHAL are integer-only, so Debug output is platform-stable.
        let _ = write!(w, "{}:{:?};", e.at.as_nanos(), e.event);
    }
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_order_sensitive() {
        let a = TraceEvent {
            at: Nanos(1),
            event: 0xAAu8,
        };
        let b = TraceEvent {
            at: Nanos(2),
            event: 0xBBu8,
        };
        let h1 = trace_hash([&a, &b]);
        let h2 = trace_hash([&a, &b]);
        let h3 = trace_hash([&b, &a]);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }
}
