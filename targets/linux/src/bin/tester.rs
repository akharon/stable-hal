//! Linux tester: drives one MODBUS RTU transaction against a real DUT over
//! a real termios serial port using the *unmodified* `RtuMaster` reference
//! driver, capturing genuine per-byte `TxStart`/`TxEnd`/`Delivered` timing
//! (unlike `targets/rp2040`'s tester, which currently emits only a skeleton
//! trace — see its README), and writes a real SHC1 file to disk.
//!
//! Usage: `linux-tester <serial-device> <capture-out-path>`
//!
//! Wire two USB-UART adapters together (TX/RX crossed, common ground); run
//! `linux-dut-slave` against one device, this tester against the other:
//!
//!   linux-dut-slave /dev/ttyUSB0 &
//!   linux-tester /dev/ttyUSB1 capture.shc1
//!   cargo run -p stablehal-certify -- judge capture.shc1 --timeout-ms 50
//!
//! must report CONFORMING for the reference DUT build.

use stablehal_contract::Nanos;
use stablehal_linux::SerialPort;
use stablehal_modbus::pdu::Request;
use stablehal_modbus::{MasterConfig, MasterProgress, RtuMaster, RtuTiming};
use stablehal_shc1::{append_end_of_observation, write_stream, Header, FLAG_SELF_REPORT, CONTRACT_RTU_V0_1};
use stablehal_uart::UartConfig;

const DUT_UNIT: u8 = 0x11;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: linux-tester <serial-device> <capture-out-path>");
    let out_path = args
        .next()
        .expect("usage: linux-tester <serial-device> <capture-out-path>");

    let uart_cfg = UartConfig::rtu_default(9600);
    let mut bus = SerialPort::open(&path, uart_cfg, 0).expect("open serial device");

    let timing = RtuTiming::from_uart(&uart_cfg, Nanos::from_millis(50));
    let mut master = RtuMaster::new(MasterConfig::new(timing));
    master.start(DUT_UNIT, &Request::ReadHoldingRegisters { addr: 0, count: 4 });

    let deadline = bus.now() + Nanos::from_secs(2);
    let result = loop {
        bus.service();
        let now = bus.now();
        match master.poll(now, &mut bus) {
            MasterProgress::Busy { .. } => {
                if now > deadline {
                    break Err("timed out waiting for DUT response".to_string());
                }
            }
            MasterProgress::Done(result) => break result.map_err(|e| format!("{e:?}")),
        }
    };

    match &result {
        Ok(adu) => eprintln!("linux-tester: transaction succeeded, {} response bytes", adu.bytes().len()),
        Err(e) => eprintln!("linux-tester: transaction failed: {e}"),
    }

    let header = Header {
        contract: CONTRACT_RTU_V0_1,
        flags: FLAG_SELF_REPORT,
        baud: uart_cfg.baud,
        uart_cfg,
    };
    let trace = bus.take_trace();
    let mut bytes = write_stream(&header, &trace);
    append_end_of_observation(&mut bytes, bus.now().as_nanos());

    std::fs::write(&out_path, &bytes).expect("write capture");
    eprintln!("linux-tester: wrote {} bytes ({} records) to {out_path}", bytes.len(), trace.len());
}
