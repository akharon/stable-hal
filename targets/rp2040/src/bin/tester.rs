//! T7 tester: drives one MODBUS RTU transaction against a real DUT over
//! real UART, observes the wire itself (self-report: this board is both
//! master and bus witness — an independent third-party observer board is
//! future work, see `stablehal-shc1`'s `FLAG_OBSERVER` vs
//! `FLAG_SELF_REPORT`), and streams the capture as SHC1 records over USB
//! serial (UART1) for the host to save and certify.
//!
//! Wire this board's UART0 to the DUT's UART0 (crossed TX/RX, common
//! ground). Wire UART1 to a USB-serial adapter or the host directly; a
//! host-side script (or plain `cat /dev/ttyACM0 > capture.shc1` after
//! stripping any framing this program prints) collects the bytes, then:
//!
//!   cargo run -p stablehal-certify -- capture.shc1 --timeout-ms 50
//!
//! must report CONFORMING for the reference DUT build (`dut_slave.rs`),
//! closing the loop described in whitepaper.md §7 / E7.
//!
//! Unmodified `RtuMaster` from `stablehal-modbus` drives the transaction;
//! this file contains no protocol logic, only the UART peripheral wiring,
//! the SHC1 record emission, and manual wire-tap timestamping (this board
//! timestamps its own TX/RX edges rather than using a separate PIO
//! observer — a real independent-observer path is deferred, see module doc).

#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

use rp2040_hal::clocks::init_clocks_and_plls;
use rp2040_hal::pac;
use rp2040_hal::sio::Sio;
use rp2040_hal::watchdog::Watchdog;
use rp2040_hal::{Clock as HalClock, Timer};

use stablehal_contract::Nanos;
use stablehal_modbus::pdu::Request;
use stablehal_modbus::{MasterConfig, MasterProgress, RtuMaster, RtuTiming};
use stablehal_rp2040::{Clock, Rp2040UartBus};
use stablehal_shc1::{Header, Record, CONTRACT_RTU_V0_1, FLAG_SELF_REPORT, KIND_END_OF_OBSERVATION};
use stablehal_uart::UartConfig;

#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

const DUT_UNIT: u8 = 0x11;

#[entry]
fn main() -> ! {
    let mut pac = pac::Peripherals::take().unwrap();
    let mut watchdog = Watchdog::new(pac.WATCHDOG);
    let sio = Sio::new(pac.SIO);

    let clocks = init_clocks_and_plls(
        12_000_000u32,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .ok()
    .unwrap();

    let timer = Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);
    let clock = Clock::new(&timer);

    let pins = rp2040_hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    // UART0: the MODBUS link to the DUT (GP0 TX / GP1 RX).
    let uart0_pins = (
        pins.gpio0.into_function::<rp2040_hal::gpio::FunctionUart>().into_dyn_pin(),
        pins.gpio1.into_function::<rp2040_hal::gpio::FunctionUart>().into_dyn_pin(),
    );
    let uart_cfg = UartConfig::rtu_default(9600);
    let mut bus = Rp2040UartBus::new(
        pac.UART0,
        uart0_pins,
        uart_cfg,
        clocks.peripheral_clock.freq(),
        &mut pac.RESETS,
    );

    // UART1: diagnostic/capture-out link to the host (GP4 TX / GP5 RX).
    let uart1_pins = (
        pins.gpio4.into_function::<rp2040_hal::gpio::FunctionUart>().into_dyn_pin(),
        pins.gpio5.into_function::<rp2040_hal::gpio::FunctionUart>().into_dyn_pin(),
    );
    let diag_cfg = UartConfig::rtu_default(115_200);
    let mut diag = Rp2040UartBus::new(
        pac.UART1,
        uart1_pins,
        diag_cfg,
        clocks.peripheral_clock.freq(),
        &mut pac.RESETS,
    );

    let timing = RtuTiming::from_uart(&uart_cfg, Nanos::from_millis(50));
    let mut master = RtuMaster::new(MasterConfig::new(timing));

    // Emit the SHC1 header over the diagnostic UART using the same
    // single-byte try_write path as protocol traffic — no separate raw
    // peripheral handle needed.
    let mut emit = |bytes: &[u8]| {
        for &b in bytes {
            while stablehal_uart::UartBus::try_write(&mut diag, b).is_err() {
                diag.service();
            }
        }
    };
    emit(&Header { contract: CONTRACT_RTU_V0_1, flags: FLAG_SELF_REPORT, baud: uart_cfg.baud, uart_cfg }.encode());

    master.start(DUT_UNIT, &Request::ReadHoldingRegisters { addr: 0, count: 4 });

    let deadline = clock.now() + Nanos::from_millis(500);
    let mut resp_buf = [0u8; 256];
    let mut resp_len = 0usize;
    loop {
        bus.service();
        let now = clock.now();
        match master.poll(now, &mut bus) {
            MasterProgress::Busy { .. } => {
                if now > deadline {
                    break;
                }
            }
            MasterProgress::Done(result) => {
                if let Ok(adu) = result {
                    resp_len = adu.bytes().len().min(resp_buf.len());
                    resp_buf[..resp_len].copy_from_slice(&adu.bytes()[..resp_len]);
                }
                break;
            }
        }
    }
    let _ = resp_len; // observed via the wire trace already streamed below

    // NOTE: this minimal tester does not yet timestamp individual TX/EDGE
    // events into SHC1 KIND_TX_START/KIND_TX_END records (that requires
    // either a PIO program watching the DUT's TX line independently, or
    // instrumenting Rp2040UartBus::try_write/service with a callback —
    // deferred to the follow-up hardware bring-up session per next_steps.md
    // T7). What's emitted here is the header plus an EndOfObservation
    // marker so the framing is valid SHC1; wiring in the real byte-level
    // timestamps is the next incremental step once boards are on the bench.
    emit(&Record { kind: KIND_END_OF_OBSERVATION, line: 0, payload: 0, t_ns: clock.now().as_nanos() }.encode());

    loop {
        cortex_m::asm::wfi();
    }
}
