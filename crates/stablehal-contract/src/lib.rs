//! StableHAL contract kernel.
//!
//! A *contract* is an executable specification of a HAL boundary: a state
//! machine over timestamped events, whose timing obligations are first-class
//! terms. Real hardware nondeterminism (latency ranges, faults) is expressed
//! as *bounded nondeterminism* inside the contract, never left implicit.
//!
//! One contract yields three artifacts:
//! * a reference simulation that is conforming by construction,
//! * a runtime monitor ([`oracle::Oracle`]) that judges any trace — simulated
//!   or captured from hardware — against the same clauses,
//! * a conformance suite generated from the clause list.
//!
//! Everything in this crate is `no_std` so the identical oracle runs on host
//! and on target.

#![no_std]
#![deny(unsafe_code)]

pub mod clause;
pub mod measure;
pub mod oracle;
pub mod prng;
pub mod time;
pub mod trace;

pub use clause::Clause;
pub use measure::{ClauseHull, EnvelopeAccumulator, Measurement};
pub use oracle::{Breach, Contract, Oracle, Verdict, Violation};
pub use prng::SplitMix64;
pub use time::{Nanos, TimeBound};
pub use trace::TraceEvent;
