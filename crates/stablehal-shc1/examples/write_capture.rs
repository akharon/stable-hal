//! Writes a conforming reference-slave run to `out.shc1` — usage demo for
//! the SHC1 writer, and a fixture for exercising `stablehal-certify` before
//! real hardware exists (WP3 §3.1/3.2).

use stablehal_conformance::runner::{
    rtu_observer, OwnedRequest, ScriptedMaster, SlaveActor, Txn, Verify,
};
use stablehal_conformance::{pattern, DUT_UNIT};
use stablehal_contract::Nanos;
use stablehal_modbus::{MasterConfig, RegisterBank, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_shc1::{append_end_of_observation, write_stream, Header, FLAG_OBSERVER};
use stablehal_sim::{Sim, SimConfig};
use stablehal_uart::UartConfig;

fn main() {
    let uart = UartConfig::rtu_default(9600);
    let timing = RtuTiming::from_uart(&uart, Nanos::from_millis(50));

    let (master, _outcome) = ScriptedMaster::new(
        MasterConfig::new(timing),
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
            timing,
            response_delay: Nanos::from_millis(1),
            bugs: SlaveBugs::default(),
        },
        RegisterBank::with_pattern(pattern),
    );

    let mut sim = Sim::new(
        SimConfig::new(0xC0FFEE, uart),
        Box::new(master),
        Box::new(slave),
    );
    sim.add_observer(rtu_observer(timing, true, true));
    sim.run_until(Nanos::from_millis(200))
        .expect("conforming reference run");

    let header = Header {
        contract: stablehal_shc1::CONTRACT_RTU_V0_1,
        flags: FLAG_OBSERVER,
        baud: uart.baud,
        uart_cfg: uart,
    };
    let mut bytes = write_stream(&header, sim.trace());
    let end_at = sim.trace().last().map(|e| e.at.as_nanos() + 1).unwrap_or(0);
    append_end_of_observation(&mut bytes, end_at);

    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "out.shc1".to_string());
    std::fs::write(&out_path, &bytes).expect("write capture");
    println!("wrote {} bytes to {out_path}", bytes.len());
}
