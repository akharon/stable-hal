//! Linux DUT: the unmodified `RtuSlave` driver from `stablehal-modbus`,
//! running on a real termios serial port instead of the simulator's virtual
//! wire — the std-side counterpart of `targets/rp2040`'s `dut_slave.rs`.
//!
//! No protocol logic lives in this file — it only wires the real serial fd
//! to `UartBus` and drives `RtuSlave::poll` in a loop.
//!
//! Usage: `linux-dut-slave <serial-device>` (e.g. `/dev/ttyUSB0`)

use stablehal_contract::Nanos;
use stablehal_linux::SerialPort;
use stablehal_modbus::{RegisterBank, RtuSlave, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_uart::UartConfig;

/// DUT unit address — must match `DUT_UNIT` in the tester fixture and
/// `stablehal_conformance`.
const DUT_UNIT: u8 = 0x11;

/// Reference register pattern — must match `stablehal_conformance::pattern`
/// so a readback check against this DUT passes.
fn pattern(addr: u16) -> u16 {
    addr.wrapping_mul(7).wrapping_add(1)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: linux-dut-slave <serial-device>");

    let uart_cfg = UartConfig::rtu_default(9600);
    let mut bus = SerialPort::open(&path, uart_cfg, 1).expect("open serial device");

    let timing = RtuTiming::from_uart(&uart_cfg, Nanos::from_millis(50));
    let mut slave = RtuSlave::new(
        SlaveConfig {
            unit: DUT_UNIT,
            timing,
            response_delay: Nanos::from_millis(1),
            bugs: SlaveBugs::default(), // conforming reference — must pass every case
        },
        RegisterBank::with_pattern(pattern),
    );

    eprintln!("linux-dut-slave: listening on {path} as unit 0x{DUT_UNIT:02x}");
    loop {
        bus.service();
        let now = bus.now();
        let _progress = slave.poll(now, &mut bus);
    }
}
