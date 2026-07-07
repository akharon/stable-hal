//! The trace oracle: folds timestamped events through a contract state
//! machine and yields either conformance or a violation that names the
//! broken clause. The same oracle judges simulated traces and traces
//! captured from real hardware — that is what closes the loop.

use core::fmt;

use crate::clause::Clause;
use crate::time::{Nanos, TimeBound};
use crate::trace::TraceEvent;

/// A broken obligation, as detected inside a contract step.
#[derive(Copy, Clone, Debug)]
pub struct Breach {
    pub clause: &'static Clause,
    /// The measured duration, when the clause is a timing obligation.
    pub measured: Option<Nanos>,
    /// The envelope the measurement had to fall in.
    pub required: Option<TimeBound>,
}

impl Breach {
    pub const fn of(clause: &'static Clause) -> Self {
        Breach {
            clause,
            measured: None,
            required: None,
        }
    }
    pub const fn timing(clause: &'static Clause, measured: Nanos, required: TimeBound) -> Self {
        Breach {
            clause,
            measured: Some(measured),
            required: Some(required),
        }
    }
}

/// An executable interface contract: a deterministic state machine over
/// timestamped events. `step` either advances the state or reports the
/// breached clause. Determinism requirement: for a given (state, at, event)
/// the result must be a pure function — no interior randomness, no wall time.
pub trait Contract {
    type State: Clone + fmt::Debug;
    type Event: Clone + fmt::Debug;

    fn initial(&self) -> Self::State;
    fn step(&self, state: &mut Self::State, at: Nanos, event: &Self::Event) -> Result<(), Breach>;
    /// Every clause this contract can enforce. Conformance generation and
    /// coverage reporting enumerate this list.
    fn clauses(&self) -> &'static [&'static Clause];
}

/// A violation verdict: the breach plus where in the trace it happened.
#[derive(Clone, Debug)]
pub struct Violation<E: Clone + fmt::Debug> {
    pub breach: Breach,
    pub at: Nanos,
    /// 0-based index of the offending event in the observed sequence.
    pub step: usize,
    pub event: E,
}

impl<E: Clone + fmt::Debug> fmt::Display for Violation<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "clause {} violated at {} (step {}): {} [{}]",
            self.breach.clause.id,
            self.at,
            self.step,
            self.breach.clause.text,
            self.breach.clause.citation
        )?;
        if let (Some(m), Some(r)) = (self.breach.measured, self.breach.required) {
            write!(f, " — measured {}, required {}", m, r)?;
        }
        write!(f, " — offending event: {:?}", self.event)
    }
}

/// Online monitor over one contract. Feed it events as they happen (in the
/// simulator, or from a capture on target); it halts at the first violation.
pub struct Oracle<C: Contract> {
    contract: C,
    state: C::State,
    steps: usize,
}

impl<C: Contract> Oracle<C> {
    pub fn new(contract: C) -> Self {
        let state = contract.initial();
        Oracle {
            contract,
            state,
            steps: 0,
        }
    }

    pub fn observe(&mut self, at: Nanos, event: &C::Event) -> Result<(), Violation<C::Event>> {
        let step = self.steps;
        self.steps += 1;
        self.contract
            .step(&mut self.state, at, event)
            .map_err(|breach| Violation {
                breach,
                at,
                step,
                event: event.clone(),
            })
    }

    pub fn steps(&self) -> usize {
        self.steps
    }
    pub fn state(&self) -> &C::State {
        &self.state
    }
    pub fn contract(&self) -> &C {
        &self.contract
    }
}

/// Verdict over a complete recorded trace.
#[derive(Clone, Debug)]
pub enum Verdict<E: Clone + fmt::Debug> {
    Conforming { steps: usize },
    Violated(Violation<E>),
}

impl<E: Clone + fmt::Debug> Verdict<E> {
    pub fn is_conforming(&self) -> bool {
        matches!(self, Verdict::Conforming { .. })
    }
}

/// Judge a complete trace offline. Used for replay files and hardware captures.
pub fn check_trace<C: Contract>(contract: C, trace: &[TraceEvent<C::Event>]) -> Verdict<C::Event> {
    let mut oracle = Oracle::new(contract);
    for ev in trace {
        if let Err(v) = oracle.observe(ev.at, &ev.event) {
            return Verdict::Violated(v);
        }
    }
    Verdict::Conforming {
        steps: oracle.steps(),
    }
}
