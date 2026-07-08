//! T7 device-under-test: the unmodified `RtuSlave` driver from
//! `stablehal-modbus`, running on real RP2040 UART instead of the
//! simulator's virtual wire. Flash this to one RP Zero; wire its UART0
//! TX/RX to the tester board's UART0 RX/TX (crossed) plus common ground.
//!
//! No protocol logic lives in this file — it only wires the real UART
//! peripheral to `UartBus` and drives `RtuSlave::poll` in a loop. This is
//! the property the whitepaper's §7 depends on: identical driver code,
//! simulated vs. on-target.

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
use stablehal_modbus::{RegisterBank, RtuSlave, RtuTiming, SlaveBugs, SlaveConfig};
use stablehal_rp2040::{Clock, Rp2040UartBus};
use stablehal_uart::UartConfig;

/// Boot2 stage required by every RP2040 binary (W25Q-family flash bootloader).
#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

/// DUT unit address — must match `DUT_UNIT` in `stablehal_conformance`.
const DUT_UNIT: u8 = 0x11;

/// Reference register pattern — must match `stablehal_conformance::pattern`
/// so the tester's readback verification passes.
fn pattern(addr: u16) -> u16 {
    addr.wrapping_mul(7).wrapping_add(1)
}

#[entry]
fn main() -> ! {
    let mut pac = pac::Peripherals::take().unwrap();
    let core = pac::CorePeripherals::take().unwrap();
    let mut watchdog = Watchdog::new(pac.WATCHDOG);
    let sio = Sio::new(pac.SIO);

    let clocks = init_clocks_and_plls(
        12_000_000u32, // RP Zero / Pico crystal
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
    // GP0 = UART0 TX, GP1 = UART0 RX (standard RP2040 default UART pins).
    let uart_pins = (
        pins.gpio0.into_function::<rp2040_hal::gpio::FunctionUart>().into_dyn_pin(),
        pins.gpio1.into_function::<rp2040_hal::gpio::FunctionUart>().into_dyn_pin(),
    );

    let uart_cfg = UartConfig::rtu_default(9600);
    let mut bus = Rp2040UartBus::new(
        pac.UART0,
        uart_pins,
        uart_cfg,
        clocks.peripheral_clock.freq(),
        &mut pac.RESETS,
    );

    let timing = RtuTiming::from_uart(&uart_cfg, Nanos::from_millis(50));
    let mut slave = RtuSlave::new(
        SlaveConfig {
            unit: DUT_UNIT,
            timing,
            response_delay: Nanos::from_millis(1),
            bugs: SlaveBugs::default(), // conforming reference — this build must pass every case
        },
        RegisterBank::with_pattern(pattern),
    );

    let _core = core; // reserved for future SysTick-driven polling cadence

    loop {
        bus.service();
        let now = clock.now();
        let _progress = slave.poll(now, &mut bus);
        // No sleep: poll is cheap and non-blocking; a production build
        // would WFE on the UART RX interrupt instead of busy-polling.
    }
}
