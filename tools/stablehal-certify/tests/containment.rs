//! Envelope containment payoff test: a "hardware" capture built from the
//! same reference traffic the simulator explores must be COVERED (round-trip
//! soundness); a capture whose timing lands outside what the simulator ever
//! exercised — while still contract-conforming — must be reported UNCOVERED.
//! This is the check that has teeth: it proves containment actually rejects
//! hardware operating in an untested timing region, not just hardware that
//! breaches the contract outright.

use stablehal_certify::contain::contain;
use stablehal_certify::envelope::Envelope;
use stablehal_conformance::runner::{
    rtu_observer, sim_envelope, OwnedRequest, ScriptedMaster, SlaveActor, SuiteConfig, Txn, Verify,
};
use stablehal_conformance::{pattern, DUT_UNIT};
use stablehal_contract::oracle::check_trace_measured;
use stablehal_contract::{EnvelopeAccumulator, Nanos};
use stablehal_modbus::contract::RtuContract;
use stablehal_modbus::{MasterConfig, RegisterBank, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_shc1::{
    append_end_of_observation, parse_stream, write_stream, Header, FLAG_OBSERVER,
};
use stablehal_sim::{Sim, SimConfig};
use stablehal_uart::UartConfig;

const UART: UartConfig = UartConfig::rtu_default(9600);
const CAPACITY: usize = 8;

fn timing() -> RtuTiming {
    RtuTiming::from_uart(&UART, Nanos::from_millis(50))
}

/// Capture one read transaction as an SHC1 byte stream, with the slave
/// responding after `response_delay`.
fn capture(response_delay: Nanos) -> Vec<u8> {
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
            response_delay,
            bugs: SlaveBugs::default(),
        },
        RegisterBank::with_pattern(pattern),
    );
    let mut sim = Sim::new(
        SimConfig::new(0xC0FFEE, UART),
        Box::new(master),
        Box::new(slave),
    );
    sim.add_observer(rtu_observer(timing(), true, true));
    sim.run_until(Nanos::from_millis(200))
        .expect("reference transaction must conform");

    let header = Header {
        contract: stablehal_shc1::CONTRACT_RTU_V0_1,
        flags: FLAG_OBSERVER,
        baud: UART.baud,
        uart_cfg: UART,
    };
    let trace = sim.trace();
    let mut bytes = write_stream(&header, trace);
    let end_at = trace.last().map(|e| e.at.as_nanos() + 1).unwrap_or(0);
    append_end_of_observation(&mut bytes, end_at);
    bytes
}

fn measure_capture(bytes: &[u8]) -> Envelope {
    let (_header, events, _report) = parse_stream(bytes).expect("parse SHC1 stream");
    let contract = RtuContract::new(timing());
    let (verdict, acc) = check_trace_measured::<_, CAPACITY>(contract, &events);
    assert!(
        verdict.is_conforming(),
        "capture must conform to build an envelope: {verdict:?}"
    );
    Envelope::from_accumulator("modbus-rtu", "capture", 1, &acc)
}

fn sim_envelope_over_seeds(seeds: u64) -> Envelope {
    let mut cfg = SuiteConfig::default_9600();
    let mut acc: EnvelopeAccumulator<CAPACITY> = EnvelopeAccumulator::new();
    for seed in 0..seeds {
        cfg.seed = seed;
        let (result, run_acc) = sim_envelope(&cfg);
        assert!(
            result.is_ok(),
            "seed {seed} reference run failed: {result:?}"
        );
        acc.merge(&run_acc);
    }
    Envelope::from_accumulator("modbus-rtu", "sim", seeds, &acc)
}

#[test]
fn hardware_within_sim_envelope_is_covered() {
    let sim = sim_envelope_over_seeds(50);
    // Same 1ms-ish response delay the sim's reference slave uses by default —
    // this "hardware" behaves exactly like what the sim explored.
    let hw_bytes = capture(Nanos::from_millis(1));
    let hw = measure_capture(&hw_bytes);

    let report = contain(&hw, &sim);
    assert!(
        report.all_covered(),
        "expected COVERED, got: {:?}",
        report
            .clauses
            .iter()
            .map(|c| (c.clause_id.clone(), c.verdict))
            .collect::<Vec<_>>()
    );
}

#[test]
fn hardware_outside_sim_envelope_is_uncovered() {
    let sim = sim_envelope_over_seeds(50);
    // A response arriving 20ms after the request is still well inside the
    // contract's response_timeout (50ms), so this capture conforms — but the
    // sim's reference slave never delays anywhere near that long by default,
    // so the sim envelope never explored this region.
    let hw_bytes = capture(Nanos::from_millis(20));
    let hw = measure_capture(&hw_bytes);

    let report = contain(&hw, &sim);
    assert!(
        !report.all_covered(),
        "expected UNCOVERED (hardware operates outside the sim-explored envelope)"
    );
    let resp = report
        .clauses
        .iter()
        .find(|c| c.clause_id == "RTU-RESP-TIME")
        .expect("RTU-RESP-TIME must be measured");
    assert!(
        matches!(
            resp.verdict,
            stablehal_certify::contain::ClauseVerdict::Uncovered { .. }
        ),
        "expected RTU-RESP-TIME to be Uncovered, got {:?}",
        resp.verdict
    );
}
