//! The trace oracle: folds timestamped events through a contract state
//! machine and yields either conformance or a violation that names the
//! broken clause. The same oracle judges simulated traces and traces
//! captured from real hardware — that is what closes the loop.

use core::fmt;

use crate::clause::Clause;
use crate::measure::{EnvelopeAccumulator, Measurement};
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

    /// Like [`Contract::step`], but also reports every timing duration it
    /// computes — passing or not — to `sink`, for envelope-containment
    /// accounting. Default delegates to `step` with no measurements, so
    /// existing contracts keep compiling; a contract opts in by overriding
    /// this and having `step` delegate to it (single judging path, no
    /// drift between the two).
    fn step_measured(
        &self,
        state: &mut Self::State,
        at: Nanos,
        event: &Self::Event,
        sink: &mut dyn FnMut(Measurement),
    ) -> Result<(), Breach> {
        let _ = sink;
        self.step(state, at, event)
    }
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

/// Like [`check_trace`], but also folds every timing measurement the
/// contract reports into an [`EnvelopeAccumulator`] — the observed hull for
/// this trace. Judging still halts at the first violation, same as
/// `check_trace`; the accumulator reflects whatever was observed up to that
/// point.
pub fn check_trace_measured<C: Contract, const N: usize>(
    contract: C,
    trace: &[TraceEvent<C::Event>],
) -> (Verdict<C::Event>, EnvelopeAccumulator<N>) {
    let mut state = contract.initial();
    let mut acc: EnvelopeAccumulator<N> = EnvelopeAccumulator::new();
    for (step, ev) in trace.iter().enumerate() {
        let mut sink = |m: Measurement| acc.observe(m);
        if let Err(breach) = contract.step_measured(&mut state, ev.at, &ev.event, &mut sink) {
            let v = Violation {
                breach,
                at: ev.at,
                step,
                event: ev.event.clone(),
            };
            return (Verdict::Violated(v), acc);
        }
    }
    (Verdict::Conforming { steps: trace.len() }, acc)
}
