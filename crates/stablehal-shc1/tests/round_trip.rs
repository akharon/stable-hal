//! WP3 §3.2 round-trip test: for a conforming run and for every
//! `SlaveBugs` catalog entry, the sim-side oracle verdict and the
//! certifier-from-capture verdict (parse SHC1 -> RtuContract oracle) must
//! agree — proving the SHC1 format and RTU mapping before any firmware
//! exists.

use stablehal_conformance::runner::{
    rtu_observer, OwnedRequest, ScriptedMaster, SlaveActor, Txn, Verify,
};
use stablehal_conformance::{pattern, DUT_UNIT};
use stablehal_contract::oracle::check_trace;
use stablehal_contract::{Nanos, Verdict};
use stablehal_modbus::contract::RtuContract;
use stablehal_modbus::{MasterConfig, RegisterBank, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_shc1::{
    append_end_of_observation, parse_stream, write_stream, Header, FLAG_OBSERVER,
};
use stablehal_sim::{Sim, SimConfig};
use stablehal_uart::UartConfig;

const UART: UartConfig = UartConfig::rtu_default(9600);

fn timing() -> RtuTiming {
    RtuTiming::from_uart(&UART, Nanos::from_millis(50))
}

/// Run one transaction against a slave with the given bugs; return the sim
/// wire trace (what the SHC1 writer will serialize) and the sim-side
/// verdict for comparison.
fn run(
    bugs: SlaveBugs,
) -> (
    Vec<stablehal_contract::TraceEvent<(u8, stablehal_uart::LineEvent)>>,
    bool,
) {
    let (master, _outcome) = ScriptedMaster::new(
        MasterConfig::new(timing()),
        vec![Txn {
            unit: DUT_UNIT,
            request: OwnedRequest::ReadHolding { addr: 0, count: 4 },
            verify: Verify::Pattern { addr: 0 },
            needs_prev_ok: false,
        }],
        pattern,
    );
    let slave = SlaveActor::new(
        SlaveConfig {
            unit: DUT_UNIT,
            timing: timing(),
            response_delay: Nanos::from_millis(1),
            bugs,
        },
        RegisterBank::with_pattern(pattern),
    );

    let mut sim = Sim::new(
        SimConfig::new(0xC0FFEE, UART),
        Box::new(master),
        Box::new(slave),
    );
    sim.add_observer(rtu_observer(timing(), true, true));
    let sim_ok = sim.run_until(Nanos::from_millis(200)).is_ok();
    (sim.trace().to_vec(), sim_ok)
}

fn certify_verdict(
    trace: &[stablehal_contract::TraceEvent<(u8, stablehal_uart::LineEvent)>],
) -> bool {
    let header = Header {
        contract: stablehal_shc1::CONTRACT_RTU_V0_1,
        flags: FLAG_OBSERVER,
        baud: UART.baud,
        uart_cfg: UART,
    };
    let mut bytes = write_stream(&header, trace);
    let end_at = trace.last().map(|e| e.at.as_nanos() + 1).unwrap_or(0);
    append_end_of_observation(&mut bytes, end_at);

    let (_header, events, _report) = parse_stream(&bytes).expect("parse SHC1 stream");
    let verdict = check_trace(RtuContract::new(timing()), &events);
    matches!(verdict, Verdict::Conforming { .. })
}

fn assert_round_trip(name: &str, bugs: SlaveBugs) {
    let (trace, sim_ok) = run(bugs);
    let cert_ok = certify_verdict(&trace);
    assert_eq!(
        sim_ok, cert_ok,
        "{name}: sim verdict ({sim_ok}) disagrees with certifier verdict ({cert_ok})"
    );
}

#[test]
fn conforming_reference_round_trips() {
    assert_round_trip("reference (conforming)", SlaveBugs::default());
}

#[test]
fn every_cataloged_bug_round_trips() {
    let catalog: Vec<(&str, SlaveBugs)> = vec![
        (
            "responds inside t3.5",
            SlaveBugs {
                respond_inside_t35: true,
                ..Default::default()
            },
        ),
        (
            "corrupt CRC",
            SlaveBugs {
                corrupt_crc: true,
                ..Default::default()
            },
        ),
        (
            "gap > t1.5 in response",
            SlaveBugs {
                intra_frame_gap: Some(timing().t15 + timing().t15),
                ..Default::default()
            },
        ),
        (
            "response after timeout",
            SlaveBugs {
                late_by: Some(Nanos::from_millis(10)),
                ..Default::default()
            },
        ),
        (
            "wrong function code",
            SlaveBugs {
                wrong_fc: true,
                ..Default::default()
            },
        ),
        (
            "wrong unit address",
            SlaveBugs {
                wrong_unit: true,
                ..Default::default()
            },
        ),
    ];
    for (name, bugs) in catalog {
        assert_round_trip(name, bugs);
    }
}
