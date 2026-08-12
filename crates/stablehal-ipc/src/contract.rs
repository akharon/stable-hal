//! The IPC channel contract: request/response sequencing, correlation,
//! scheduling-latency and delivery envelopes, backpressure discipline,
//! at-most-once delivery, ordering, and peer-death surfacing — as an
//! executable state machine over datagram-level events.
//!
//! The same contract instance judges simulated channels and captures taken
//! from a real socket. Each side can be excluded from judgement
//! (`judge_requester`/`judge_responder`) so a conformance tester may
//! deliberately misbehave while the device under test is held to every
//! clause.

use stablehal_contract::measure::Measurement;
use stablehal_contract::{Breach, Clause, Contract, Nanos};

use crate::{ChannelKind, CorrelationId, IpcConfig, IpcFault};

/// Observable events on the IPC channel. Datagram granularity: one whole
/// message is one event (see the crate doc comment on why).
///
/// Judged events are what an endpoint *emitted* (`Sent`, `Responded`), never
/// what the transport did to it — the same "transmitter truth" rule as
/// ADR-1. `Delivered` exists so delivery latency can be measured, but it is
/// never used to blame an endpoint.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IpcEvent {
    /// The requester emitted a request datagram.
    Sent { id: CorrelationId, len: u32 },
    /// The transport delivered a request to the responder.
    Delivered { id: CorrelationId },
    /// The responder emitted a response datagram.
    Responded { id: CorrelationId, len: u32 },
    /// The transport delivered a response back to the requester.
    ResponseDelivered { id: CorrelationId },
    /// A send was correctly rejected because the queue was full. This is a
    /// *conforming* observation — it proves backpressure was surfaced rather
    /// than the datagram being silently dropped.
    SendRejected { id: CorrelationId },
    /// A datagram was silently dropped by an endpoint that should have
    /// rejected it (queue full, but no `WouldBlock` surfaced). Always a
    /// breach of `IPC-BACKPRESSURE`.
    SilentDrop { id: CorrelationId },
    /// A transport-injected fault was surfaced to an endpoint.
    Fault { fault: IpcFault },
    /// The peer endpoint terminated. After this, no traffic is legal.
    PeerExit,
    /// Appended by the harness after all activity: closes any outstanding
    /// transaction (ADR-7 — a trailing unanswered request cannot escape
    /// judgement).
    EndOfObservation,
}

pub static CL_IPC_SEQ: Clause = Clause {
    id: "IPC-SEQ",
    citation: "IPC channel contract §1",
    text: "Request and response strictly alternate per correlation id; a response requires a matching outstanding request.",
};

pub static CL_IPC_RESP_TIME: Clause = Clause {
    id: "IPC-RESP-TIME",
    citation: "IPC channel contract §2",
    text:
        "A responder answers a delivered request within the configured scheduling-latency envelope.",
};

pub static CL_IPC_DELIVERY: Clause = Clause {
    id: "IPC-DELIVERY",
    citation: "IPC channel contract §3",
    text: "Transport delivery latency (send to receive) stays inside the configured envelope.",
};

pub static CL_IPC_CORRELATION: Clause = Clause {
    id: "IPC-CORRELATION",
    citation: "IPC channel contract §4",
    text: "A response carries the correlation id of the request it answers.",
};

pub static CL_IPC_MSG_MAXLEN: Clause = Clause {
    id: "IPC-MSG-MAXLEN",
    citation: "IPC channel contract §5",
    text: "No datagram exceeds the channel's configured maximum length.",
};

pub static CL_IPC_BACKPRESSURE: Clause = Clause {
    id: "IPC-BACKPRESSURE",
    citation: "IPC channel contract §6",
    text:
        "A send into a full queue must fail loudly (WouldBlock), never silently drop the datagram.",
};

pub static CL_IPC_NO_DUPLICATE: Clause = Clause {
    id: "IPC-NO-DUPLICATE",
    citation: "IPC channel contract §7",
    text: "A correlation id is answered at most once (at-most-once delivery).",
};

pub static CL_IPC_PEER_DEATH: Clause = Clause {
    id: "IPC-PEER-DEATH",
    citation: "IPC channel contract §8",
    text: "Peer termination surfaces as an explicit PeerGone fault; no traffic is legal after peer exit.",
};

pub static CL_IPC_ORDERING: Clause = Clause {
    id: "IPC-ORDERING",
    citation: "IPC channel contract §9",
    text: "Datagrams within one correlation stream are observed in send order.",
};

static IPC_CLAUSES: [&Clause; 9] = [
    &CL_IPC_SEQ,
    &CL_IPC_RESP_TIME,
    &CL_IPC_DELIVERY,
    &CL_IPC_CORRELATION,
    &CL_IPC_MSG_MAXLEN,
    &CL_IPC_BACKPRESSURE,
    &CL_IPC_NO_DUPLICATE,
    &CL_IPC_PEER_DEATH,
    &CL_IPC_ORDERING,
];

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct IpcContract {
    pub config: IpcConfig,
    /// Judge the requester's obligations (send discipline, message length,
    /// backpressure handling).
    pub judge_requester: bool,
    /// Judge the responder's obligations (response time, correlation,
    /// at-most-once).
    pub judge_responder: bool,
}

impl IpcContract {
    pub fn new(config: IpcConfig) -> Self {
        IpcContract {
            config,
            judge_requester: true,
            judge_responder: true,
        }
    }

    fn fail_requester(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_requester {
            Err(breach)
        } else {
            Ok(())
        }
    }

    fn fail_responder(&self, breach: Breach) -> Result<(), Breach> {
        if self.judge_responder {
            Err(breach)
        } else {
            Ok(())
        }
    }
}

/// How far a single correlation stream has progressed. Tracking is
/// per-transaction because IPC — unlike a half-duplex wire bus — has no
/// global "the bus is busy" state; two correlation ids may legitimately be
/// in flight at once.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TxnPhase {
    /// Sent, not yet delivered to the responder.
    Sent { at: Nanos },
    /// Delivered to the responder, awaiting its answer.
    Delivered { at: Nanos },
    /// Responder answered; awaiting delivery back to the requester.
    Answered,
    /// Fully complete. Retained so a second answer trips
    /// `IPC-NO-DUPLICATE` rather than being read as a fresh transaction.
    Complete,
}

/// Max concurrently-tracked correlation ids. Fixed capacity keeps the
/// contract `no_std` and alloc-free (G1); exceeding it is itself reported as
/// a sequencing breach rather than silently forgetting a transaction.
pub const MAX_TRACKED: usize = 16;

#[derive(Clone, Debug)]
pub struct IpcState {
    txns: [Option<(CorrelationId, TxnPhase)>; MAX_TRACKED],
    /// Highest correlation id sent so far — ordering within a stream is
    /// checked against this (`IPC-ORDERING`).
    high_water: Option<CorrelationId>,
    /// Set once `PeerExit` is observed; all later traffic is a breach.
    peer_gone: bool,
    /// Set once a terminal fault surfaced, so the harness's trailing
    /// `EndOfObservation` does not additionally blame outstanding
    /// transactions that the peer's death legitimately abandoned.
    terminal_fault: bool,
}

impl IpcState {
    fn find(&mut self, id: CorrelationId) -> Option<&mut TxnPhase> {
        self.txns
            .iter_mut()
            .flatten()
            .find(|(i, _)| *i == id)
            .map(|(_, p)| p)
    }

    fn insert(&mut self, id: CorrelationId, phase: TxnPhase) -> bool {
        // Reuse a completed slot before claiming a free one, so long runs of
        // sequential transactions do not exhaust capacity.
        if let Some(slot) = self
            .txns
            .iter_mut()
            .find(|s| matches!(s, Some((_, TxnPhase::Complete)) | None))
        {
            *slot = Some((id, phase));
            true
        } else {
            false
        }
    }
}

impl Contract for IpcContract {
    type State = IpcState;
    type Event = IpcEvent;

    fn initial(&self) -> IpcState {
        IpcState {
            txns: [None; MAX_TRACKED],
            high_water: None,
            peer_gone: false,
            terminal_fault: false,
        }
    }

    fn step(&self, state: &mut IpcState, at: Nanos, event: &IpcEvent) -> Result<(), Breach> {
        self.step_measured(state, at, event, &mut |_| {})
    }

    /// Envelope-containment side channel (ADR-11): reports every timing
    /// duration it computes — passing or not — *before* checking the bound,
    /// so `step` and `step_measured` share exactly one judging path.
    ///
    /// Measures `IPC-DELIVERY` (send → delivery) and `IPC-RESP-TIME`
    /// (delivery → response). These are the two clauses whose envelopes are
    /// dominated by scheduler behavior, so they are precisely the ones where
    /// a containment check earns its keep: it answers "did the simulation
    /// actually explore the latency region this deployment operates in?"
    fn step_measured(
        &self,
        state: &mut IpcState,
        at: Nanos,
        event: &IpcEvent,
        sink: &mut dyn FnMut(Measurement),
    ) -> Result<(), Breach> {
        // Nothing but EndOfObservation is legal once the peer is gone.
        if state.peer_gone && !matches!(event, IpcEvent::EndOfObservation) {
            return self.fail_requester(Breach::of(&CL_IPC_PEER_DEATH));
        }

        match *event {
            IpcEvent::Sent { id, len } => {
                if len > self.config.max_msg_len {
                    self.fail_requester(Breach::of(&CL_IPC_MSG_MAXLEN))?;
                }

                // Ordering: within a correlation stream, ids must not go
                // backwards. Only meaningful where the transport can
                // actually reorder — a local kernel queue cannot, so
                // claiming to test this locally would be a false coverage
                // claim (see ChannelKind::may_reorder).
                if self.config.kind.may_reorder() {
                    if let Some(hw) = state.high_water {
                        if id < hw {
                            self.fail_requester(Breach::of(&CL_IPC_ORDERING))?;
                        }
                    }
                }
                if state.high_water.is_none_or(|hw| id > hw) {
                    state.high_water = Some(id);
                }

                // A resend of an id still in flight breaks alternation.
                if let Some(phase) = state.find(id) {
                    if !matches!(phase, TxnPhase::Complete) {
                        self.fail_requester(Breach::of(&CL_IPC_SEQ))?;
                    }
                    *phase = TxnPhase::Sent { at };
                } else if !state.insert(id, TxnPhase::Sent { at }) {
                    // Capacity exhausted: too many concurrent transactions
                    // to judge honestly. Reported, never silently dropped.
                    self.fail_requester(Breach::of(&CL_IPC_SEQ))?;
                }
                Ok(())
            }

            IpcEvent::Delivered { id } => {
                let delivery_bound = self.config.delivery;
                match state.find(id) {
                    Some(TxnPhase::Sent { at: sent_at }) => {
                        let latency = at.saturating_sub(*sent_at);
                        sink(Measurement {
                            clause_id: CL_IPC_DELIVERY.id,
                            value: latency,
                        });
                        if !delivery_bound.contains(latency) {
                            self.fail_requester(Breach::timing(
                                &CL_IPC_DELIVERY,
                                latency,
                                delivery_bound,
                            ))?;
                        }
                        *state.find(id).unwrap() = TxnPhase::Delivered { at };
                        Ok(())
                    }
                    // Delivery of something never sent, or delivered twice.
                    Some(_) => self.fail_requester(Breach::of(&CL_IPC_SEQ)),
                    None => self.fail_requester(Breach::of(&CL_IPC_SEQ)),
                }
            }

            IpcEvent::Responded { id, len } => {
                if len > self.config.max_msg_len {
                    self.fail_responder(Breach::of(&CL_IPC_MSG_MAXLEN))?;
                }
                let resp_bound = self.config.resp_time;
                match state.find(id) {
                    Some(TxnPhase::Delivered { at: delivered_at }) => {
                        let turnaround = at.saturating_sub(*delivered_at);
                        sink(Measurement {
                            clause_id: CL_IPC_RESP_TIME.id,
                            value: turnaround,
                        });
                        if !resp_bound.contains(turnaround) {
                            self.fail_responder(Breach::timing(
                                &CL_IPC_RESP_TIME,
                                turnaround,
                                resp_bound,
                            ))?;
                        }
                        *state.find(id).unwrap() = TxnPhase::Answered;
                        Ok(())
                    }
                    // Answering an id that was already answered: the
                    // at-most-once violation this contract exists to catch.
                    Some(TxnPhase::Answered) | Some(TxnPhase::Complete) => {
                        self.fail_responder(Breach::of(&CL_IPC_NO_DUPLICATE))
                    }
                    // Answering before the request was delivered.
                    Some(TxnPhase::Sent { .. }) => self.fail_responder(Breach::of(&CL_IPC_SEQ)),
                    // Answering a correlation id that was never requested.
                    None => self.fail_responder(Breach::of(&CL_IPC_CORRELATION)),
                }
            }

            IpcEvent::ResponseDelivered { id } => match state.find(id) {
                Some(phase @ TxnPhase::Answered) => {
                    *phase = TxnPhase::Complete;
                    Ok(())
                }
                Some(TxnPhase::Complete) => self.fail_responder(Breach::of(&CL_IPC_NO_DUPLICATE)),
                Some(_) => self.fail_responder(Breach::of(&CL_IPC_SEQ)),
                None => self.fail_responder(Breach::of(&CL_IPC_CORRELATION)),
            },

            // Backpressure surfaced correctly: the conforming outcome.
            IpcEvent::SendRejected { id } => {
                // The rejected datagram never entered the channel, so any
                // slot it claimed must be released — otherwise a later
                // legitimate resend of the same id reads as a duplicate.
                if let Some(phase) = state.find(id) {
                    *phase = TxnPhase::Complete;
                }
                Ok(())
            }

            // Backpressure mishandled: the datagram vanished.
            IpcEvent::SilentDrop { id: _ } => self.fail_requester(Breach::of(&CL_IPC_BACKPRESSURE)),

            IpcEvent::Fault { fault } => {
                if fault.is_terminal() {
                    state.terminal_fault = true;
                }
                // Duplicate/reorder faults are only possible cross-host; if
                // the transport claims one on a local queue, the *model* is
                // wrong, and saying so is more useful than passing quietly.
                match fault {
                    IpcFault::Duplicate if self.config.kind == ChannelKind::LocalQueue => {
                        self.fail_responder(Breach::of(&CL_IPC_NO_DUPLICATE))
                    }
                    IpcFault::Reordered if self.config.kind == ChannelKind::LocalQueue => {
                        self.fail_requester(Breach::of(&CL_IPC_ORDERING))
                    }
                    _ => Ok(()),
                }
            }

            IpcEvent::PeerExit => {
                // Peer death must have been surfaced as a PeerGone fault
                // before (or at) exit — a channel that just hangs is the
                // failure mode this clause exists to forbid.
                if !state.terminal_fault {
                    self.fail_responder(Breach::of(&CL_IPC_PEER_DEATH))?;
                }
                state.peer_gone = true;
                Ok(())
            }

            IpcEvent::EndOfObservation => {
                // A request left unanswered at end of observation is a
                // response-time breach unless the peer legitimately died.
                if !state.terminal_fault {
                    for slot in state.txns.iter().flatten() {
                        if let (_, TxnPhase::Delivered { at: delivered_at }) = slot {
                            let outstanding = at.saturating_sub(*delivered_at);
                            return self.fail_responder(Breach::timing(
                                &CL_IPC_RESP_TIME,
                                outstanding,
                                self.config.resp_time,
                            ));
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn clauses(&self) -> &'static [&'static Clause] {
        &IPC_CLAUSES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::Oracle;

    fn cfg() -> IpcConfig {
        IpcConfig::local_non_realtime()
    }

    /// Walk one clean transaction: send, deliver, respond, deliver back.
    fn good_txn(o: &mut Oracle<IpcContract>, id: CorrelationId, t: Nanos) -> Nanos {
        let d = t + Nanos::from_micros(1);
        o.observe(t, &IpcEvent::Sent { id, len: 16 }).unwrap();
        o.observe(d, &IpcEvent::Delivered { id }).unwrap();
        let r = d + Nanos::from_micros(50);
        o.observe(r, &IpcEvent::Responded { id, len: 16 }).unwrap();
        let rd = r + Nanos::from_micros(1);
        o.observe(rd, &IpcEvent::ResponseDelivered { id }).unwrap();
        rd
    }

    #[test]
    fn conforming_transaction_passes() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let end = good_txn(&mut o, 1, Nanos::from_micros(10));
        o.observe(end, &IpcEvent::EndOfObservation).unwrap();
    }

    #[test]
    fn many_sequential_transactions_do_not_exhaust_slot_capacity() {
        // Regression guard for the fixed-capacity txn table: completed
        // slots must be reused, or transaction MAX_TRACKED+1 would be
        // reported as an IPC-SEQ breach that never actually happened.
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let mut t = Nanos::from_micros(10);
        for id in 0..(MAX_TRACKED as u32 * 3) {
            t = good_txn(&mut o, id, t + Nanos::from_micros(10));
        }
        o.observe(t, &IpcEvent::EndOfObservation).unwrap();
    }

    #[test]
    fn slow_responder_breaches_resp_time() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let t = Nanos::from_micros(10);
        o.observe(t, &IpcEvent::Sent { id: 1, len: 16 }).unwrap();
        let d = t + Nanos::from_micros(1);
        o.observe(d, &IpcEvent::Delivered { id: 1 }).unwrap();
        // 50 ms turnaround against a 20 ms envelope: the scheduler stall
        // this whole contract exists to make visible.
        let r = d + Nanos::from_millis(50);
        let v = o
            .observe(r, &IpcEvent::Responded { id: 1, len: 16 })
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-RESP-TIME");
    }

    #[test]
    fn slow_delivery_breaches_delivery_envelope() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let t = Nanos::from_micros(10);
        o.observe(t, &IpcEvent::Sent { id: 1, len: 16 }).unwrap();
        let v = o
            .observe(t + Nanos::from_millis(1), &IpcEvent::Delivered { id: 1 })
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-DELIVERY");
    }

    #[test]
    fn response_to_unknown_id_breaches_correlation() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let v = o
            .observe(
                Nanos::from_micros(10),
                &IpcEvent::Responded { id: 99, len: 8 },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-CORRELATION");
    }

    #[test]
    fn answering_twice_breaches_no_duplicate() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let t = Nanos::from_micros(10);
        o.observe(t, &IpcEvent::Sent { id: 1, len: 16 }).unwrap();
        let d = t + Nanos::from_micros(1);
        o.observe(d, &IpcEvent::Delivered { id: 1 }).unwrap();
        let r = d + Nanos::from_micros(50);
        o.observe(r, &IpcEvent::Responded { id: 1, len: 16 })
            .unwrap();
        let v = o
            .observe(
                r + Nanos::from_micros(10),
                &IpcEvent::Responded { id: 1, len: 16 },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-NO-DUPLICATE");
    }

    #[test]
    fn oversize_datagram_breaches_maxlen() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let v = o
            .observe(
                Nanos::from_micros(10),
                &IpcEvent::Sent {
                    id: 1,
                    len: cfg().max_msg_len + 1,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-MSG-MAXLEN");
    }

    #[test]
    fn silent_drop_breaches_backpressure() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let v = o
            .observe(Nanos::from_micros(10), &IpcEvent::SilentDrop { id: 1 })
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-BACKPRESSURE");
    }

    #[test]
    fn rejected_send_is_conforming_backpressure() {
        // The positive half of G3 for IPC-BACKPRESSURE: surfacing the
        // rejection is legal and must not be flagged.
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let t = Nanos::from_micros(10);
        o.observe(t, &IpcEvent::SendRejected { id: 1 }).unwrap();
        // And the same id may then be legitimately resent.
        good_txn(&mut o, 1, t + Nanos::from_micros(10));
    }

    #[test]
    fn peer_exit_without_surfaced_fault_breaches_peer_death() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let v = o
            .observe(Nanos::from_micros(10), &IpcEvent::PeerExit)
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-PEER-DEATH");
    }

    #[test]
    fn peer_death_surfaced_then_exit_is_conforming() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let t = Nanos::from_micros(10);
        o.observe(
            t,
            &IpcEvent::Fault {
                fault: IpcFault::PeerGone,
            },
        )
        .unwrap();
        o.observe(t, &IpcEvent::PeerExit).unwrap();
        // A request abandoned by the peer's death is not blamed on the
        // responder at end of observation.
        o.observe(t + Nanos::from_micros(1), &IpcEvent::EndOfObservation)
            .unwrap();
    }

    #[test]
    fn traffic_after_peer_exit_breaches_peer_death() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let t = Nanos::from_micros(10);
        o.observe(
            t,
            &IpcEvent::Fault {
                fault: IpcFault::PeerGone,
            },
        )
        .unwrap();
        o.observe(t, &IpcEvent::PeerExit).unwrap();
        let v = o
            .observe(t + Nanos::from_micros(5), &IpcEvent::Sent { id: 7, len: 8 })
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-PEER-DEATH");
    }

    #[test]
    fn unanswered_request_is_caught_at_end_of_observation() {
        let mut o = Oracle::new(IpcContract::new(cfg()));
        let t = Nanos::from_micros(10);
        o.observe(t, &IpcEvent::Sent { id: 1, len: 16 }).unwrap();
        let d = t + Nanos::from_micros(1);
        o.observe(d, &IpcEvent::Delivered { id: 1 }).unwrap();
        let v = o
            .observe(d + Nanos::from_millis(1), &IpcEvent::EndOfObservation)
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-RESP-TIME");
    }

    #[test]
    fn backwards_id_breaches_ordering_only_cross_host() {
        // Cross-host: the transport can reorder, so the clause is live.
        let mut o = Oracle::new(IpcContract::new(IpcConfig::cross_host()));
        let t = Nanos::from_micros(10);
        o.observe(t, &IpcEvent::Sent { id: 5, len: 8 }).unwrap();
        let v = o
            .observe(t + Nanos::from_micros(1), &IpcEvent::Sent { id: 2, len: 8 })
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-ORDERING");

        // Local queue: the kernel guarantees order, so the same sequence is
        // not judged — claiming coverage there would be dishonest.
        let mut o2 = Oracle::new(IpcContract::new(IpcConfig::local_non_realtime()));
        o2.observe(t, &IpcEvent::Sent { id: 5, len: 8 }).unwrap();
        o2.observe(t + Nanos::from_micros(1), &IpcEvent::Sent { id: 2, len: 8 })
            .unwrap();
    }

    #[test]
    fn duplicate_fault_on_local_queue_is_a_model_error() {
        let mut o = Oracle::new(IpcContract::new(IpcConfig::local_non_realtime()));
        let v = o
            .observe(
                Nanos::from_micros(10),
                &IpcEvent::Fault {
                    fault: IpcFault::Duplicate,
                },
            )
            .unwrap_err();
        assert_eq!(v.breach.clause.id, "IPC-NO-DUPLICATE");
    }

    #[test]
    fn waived_responder_is_not_judged() {
        // Per-direction judgement (ADR-1): a tester may misbehave on its own
        // side while the DUT stays accountable.
        let mut c = IpcContract::new(cfg());
        c.judge_responder = false;
        let mut o = Oracle::new(c);
        o.observe(
            Nanos::from_micros(10),
            &IpcEvent::Responded { id: 99, len: 8 },
        )
        .unwrap();
    }

    #[test]
    fn clause_list_is_complete() {
        let c = IpcContract::new(cfg());
        let clauses = c.clauses();
        assert_eq!(clauses.len(), 9);
        let ids: [&str; 9] = [
            "IPC-SEQ",
            "IPC-RESP-TIME",
            "IPC-DELIVERY",
            "IPC-CORRELATION",
            "IPC-MSG-MAXLEN",
            "IPC-BACKPRESSURE",
            "IPC-NO-DUPLICATE",
            "IPC-PEER-DEATH",
            "IPC-ORDERING",
        ];
        for id in &ids {
            assert!(clauses.iter().any(|cl| cl.id == *id), "missing clause {id}");
        }
    }
}
