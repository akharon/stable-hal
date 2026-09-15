//! Merge a requester-side and a responder-side SHC1-IPC capture into one
//! time-ordered trace.
//!
//! IPC has no shared bus the way a wire pilot does: the requester and
//! responder each capture their own view of the channel, and the oracle
//! judges one interleaved trace ordered by `(at, seq)` (G2's total order).
//! Both sides timestamp against the same host's monotonic clock, so no
//! clock-skew correction is needed — this is a **local-host-only** merge;
//! a cross-host capture (`ChannelKind::CrossHost`) would need skew
//! correction this module deliberately does not attempt.

use stablehal_contract::TraceEvent;
use stablehal_ipc::contract::IpcEvent;
use stablehal_shc1::{IpcHeader, FLAG_IPC_RESPONDER_SIDE};

#[derive(Debug, PartialEq, Eq)]
pub enum MergeError {
    /// Both captures carry the same side flag — two requester captures, or
    /// two responder captures, rather than one of each.
    SameSide,
    /// The two captures declare different contract codes.
    ContractMismatch { a: u16, b: u16 },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::SameSide => write!(
                f,
                "both captures are from the same side (need one requester + one responder capture)"
            ),
            MergeError::ContractMismatch { a, b } => {
                write!(f, "contract code mismatch: {a:#06x} vs {b:#06x}")
            }
        }
    }
}

/// Merge two `(header, trace)` pairs — order of arguments does not matter —
/// into one trace, sorted by `(at, argument-order tiebreak)`. A stable sort
/// keeps ties in the order the two inputs were given, which is the closest
/// thing to a deterministic tiebreak available when two independent
/// processes' clocks happen to report the exact same nanosecond.
pub fn merge_ipc_traces(
    a: (IpcHeader, Vec<TraceEvent<IpcEvent>>),
    b: (IpcHeader, Vec<TraceEvent<IpcEvent>>),
) -> Result<Vec<TraceEvent<IpcEvent>>, MergeError> {
    let (header_a, trace_a) = a;
    let (header_b, trace_b) = b;

    if header_a.contract != header_b.contract {
        return Err(MergeError::ContractMismatch {
            a: header_a.contract,
            b: header_b.contract,
        });
    }
    let side_a = header_a.flags & FLAG_IPC_RESPONDER_SIDE;
    let side_b = header_b.flags & FLAG_IPC_RESPONDER_SIDE;
    if side_a == side_b {
        return Err(MergeError::SameSide);
    }

    let mut merged: Vec<TraceEvent<IpcEvent>> = Vec::with_capacity(trace_a.len() + trace_b.len());
    merged.extend(trace_a);
    merged.extend(trace_b);
    merged.sort_by_key(|e| e.at.as_nanos());
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::Nanos;
    use stablehal_shc1::CONTRACT_IPC_V0_1;

    fn header(responder_side: bool) -> IpcHeader {
        IpcHeader {
            contract: CONTRACT_IPC_V0_1,
            flags: if responder_side {
                FLAG_IPC_RESPONDER_SIDE
            } else {
                0
            },
        }
    }

    fn ev(at_ns: u64, event: IpcEvent) -> TraceEvent<IpcEvent> {
        TraceEvent {
            at: Nanos::from_nanos(at_ns),
            event,
        }
    }

    #[test]
    fn interleaves_two_sides_in_time_order() {
        let requester = (
            header(false),
            vec![
                ev(0, IpcEvent::Sent { id: 1, len: 4 }),
                ev(30, IpcEvent::ResponseDelivered { id: 1 }),
            ],
        );
        let responder = (
            header(true),
            vec![
                ev(10, IpcEvent::Delivered { id: 1 }),
                ev(20, IpcEvent::Responded { id: 1, len: 4 }),
            ],
        );
        let merged = merge_ipc_traces(requester, responder).unwrap();
        let ats: Vec<u64> = merged.iter().map(|e| e.at.as_nanos()).collect();
        assert_eq!(ats, vec![0, 10, 20, 30]);
    }

    #[test]
    fn argument_order_is_irrelevant() {
        let requester = (header(false), vec![ev(5, IpcEvent::Sent { id: 1, len: 1 })]);
        let responder = (header(true), vec![ev(1, IpcEvent::Delivered { id: 1 })]);
        let a = merge_ipc_traces(requester.clone(), responder.clone()).unwrap();
        let b = merge_ipc_traces(responder, requester).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn two_requester_captures_are_rejected() {
        let a = (header(false), vec![]);
        let b = (header(false), vec![]);
        assert_eq!(merge_ipc_traces(a, b).unwrap_err(), MergeError::SameSide);
    }

    #[test]
    fn mismatched_contract_codes_are_rejected() {
        let a = (
            IpcHeader {
                contract: 0x0001,
                flags: 0,
            },
            vec![],
        );
        let b = (header(true), vec![]);
        assert_eq!(
            merge_ipc_traces(a, b).unwrap_err(),
            MergeError::ContractMismatch {
                a: 0x0001,
                b: CONTRACT_IPC_V0_1
            }
        );
    }
}
