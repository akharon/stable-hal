//! SHC1-IPC round-trip tests: header/record codec correctness and the
//! resync-on-corruption behavior, mirroring `tests/round_trip.rs`'s RTU
//! coverage but at datagram granularity. Also proves the write_stream_ipc ->
//! parse_stream_ipc round trip agrees with the oracle exactly as the RTU
//! capture path does (`check_trace` on the recovered `IpcEvent` trace).

use stablehal_contract::oracle::check_trace;
use stablehal_contract::{Nanos, TraceEvent, Verdict};
use stablehal_ipc::contract::{IpcContract, IpcEvent};
use stablehal_ipc::{IpcConfig, IpcFault};
use stablehal_shc1::{
    parse_stream_ipc, write_stream_ipc, IpcHeader, CONTRACT_IPC_V0_1, FLAG_IPC_RESPONDER_SIDE,
};

fn header() -> IpcHeader {
    IpcHeader {
        contract: CONTRACT_IPC_V0_1,
        flags: 0,
    }
}

#[test]
fn header_round_trips() {
    let h = IpcHeader {
        contract: CONTRACT_IPC_V0_1,
        flags: FLAG_IPC_RESPONDER_SIDE,
    };
    let encoded = h.encode();
    let decoded = IpcHeader::decode(&encoded).unwrap();
    assert_eq!(decoded, h);
}

#[test]
fn every_event_kind_and_fault_round_trips_through_write_and_parse() {
    let all_faults = [
        IpcFault::PeerGone,
        IpcFault::WouldBlock,
        IpcFault::Truncated,
        IpcFault::Duplicate,
        IpcFault::Reordered,
    ];
    let mut trace: Vec<TraceEvent<IpcEvent>> = vec![
        TraceEvent {
            at: Nanos::from_nanos(1),
            event: IpcEvent::Sent { id: 1, len: 16 },
        },
        TraceEvent {
            at: Nanos::from_nanos(2),
            event: IpcEvent::Delivered { id: 1 },
        },
        TraceEvent {
            at: Nanos::from_nanos(3),
            event: IpcEvent::Responded { id: 1, len: 16 },
        },
        TraceEvent {
            at: Nanos::from_nanos(4),
            event: IpcEvent::ResponseDelivered { id: 1 },
        },
        TraceEvent {
            at: Nanos::from_nanos(5),
            event: IpcEvent::SendRejected { id: 2 },
        },
        TraceEvent {
            at: Nanos::from_nanos(6),
            event: IpcEvent::SilentDrop { id: 3 },
        },
        TraceEvent {
            at: Nanos::from_nanos(7),
            event: IpcEvent::PeerExit,
        },
    ];
    for (i, fault) in all_faults.into_iter().enumerate() {
        trace.push(TraceEvent {
            at: Nanos::from_nanos(8 + i as u64),
            event: IpcEvent::Fault { fault },
        });
    }
    trace.push(TraceEvent {
        at: Nanos::from_nanos(20),
        event: IpcEvent::EndOfObservation,
    });

    let bytes = write_stream_ipc(&header(), &trace);
    let (_h, events, report) = parse_stream_ipc(&bytes).unwrap();
    assert_eq!(events, trace);
    assert_eq!(report.capture_gaps, 0);
}

fn good_trace() -> Vec<TraceEvent<IpcEvent>> {
    vec![
        TraceEvent {
            at: Nanos::from_micros(10),
            event: IpcEvent::Sent { id: 1, len: 16 },
        },
        TraceEvent {
            at: Nanos::from_micros(11),
            event: IpcEvent::Delivered { id: 1 },
        },
        TraceEvent {
            at: Nanos::from_micros(61),
            event: IpcEvent::Responded { id: 1, len: 16 },
        },
        TraceEvent {
            at: Nanos::from_micros(62),
            event: IpcEvent::ResponseDelivered { id: 1 },
        },
        TraceEvent {
            at: Nanos::from_micros(63),
            event: IpcEvent::EndOfObservation,
        },
    ]
}

#[test]
fn write_then_parse_round_trips_a_conforming_trace_through_the_oracle() {
    let trace = good_trace();
    let bytes = write_stream_ipc(&header(), &trace);
    let (parsed_header, events, report) = parse_stream_ipc(&bytes).unwrap();
    assert_eq!(parsed_header, header());
    assert_eq!(events, trace);
    assert_eq!(report.records, trace.len());
    assert_eq!(report.capture_gaps, 0);

    let verdict = check_trace(IpcContract::new(IpcConfig::local_non_realtime()), &events);
    assert!(matches!(verdict, Verdict::Conforming { .. }));
}

#[test]
fn peer_gone_fault_round_trips_through_write_and_parse() {
    let trace = vec![
        TraceEvent {
            at: Nanos::from_micros(1),
            event: IpcEvent::Fault {
                fault: IpcFault::PeerGone,
            },
        },
        TraceEvent {
            at: Nanos::from_micros(1),
            event: IpcEvent::PeerExit,
        },
        TraceEvent {
            at: Nanos::from_micros(2),
            event: IpcEvent::EndOfObservation,
        },
    ];
    let bytes = write_stream_ipc(&header(), &trace);
    let (_h, events, _report) = parse_stream_ipc(&bytes).unwrap();
    assert_eq!(events, trace);
}

#[test]
fn resync_skips_corrupt_bytes_and_counts_gaps() {
    let good_trace = vec![
        TraceEvent {
            at: Nanos::from_nanos(0),
            event: IpcEvent::Sent { id: 1, len: 16 },
        },
        TraceEvent {
            at: Nanos::from_nanos(1000),
            event: IpcEvent::EndOfObservation,
        },
    ];
    let mut stream = write_stream_ipc(&header(), &good_trace[..1]);
    // Corrupt garbage injected between two valid records.
    stream.extend_from_slice(&[0x00, 0x11, 0x22, 0x33]);
    let second = write_stream_ipc(&header(), &good_trace[1..]);
    // Drop the second stream's own header — only its one record is wanted.
    stream.extend_from_slice(&second[stablehal_shc1::IPC_HEADER_LEN..]);

    let (_h, events, report) = parse_stream_ipc(&stream).unwrap();
    assert_eq!(events, good_trace);
    assert_eq!(report.records, 2);
    assert_eq!(report.capture_gaps, 4);
}
