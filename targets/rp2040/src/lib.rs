//! RP2040 target glue: `UartBus` over the on-chip PL011 UART, plus a
//! monotonic-timer -> `Nanos` bridge. `no_std`, no allocator — everything is
//! fixed-capacity (T7 / WP3.3).
//!
//! This crate deliberately contains *no* protocol logic: `RtuSlave` and the
//! conformance runner are unmodified `no_std` code from the host workspace,
//! driven here only through the `UartBus` trait — exactly the guarantee the
//! whitepaper's §7 depends on (the identical driver code runs simulated and
//! on-target).
//!
//! UNVERIFIED: written without `thumbv6m-none-eabi` installed or hardware
//! on hand this session, so `rp2040_hal::uart::UartPeripheral` method names
//! (`read_raw`, `write_raw`, `uart_is_writable`) are from memory of the
//! 0.12 API and may not match exactly — expect to fix compile errors on
//! first build. See `targets/rp2040/README.md` for the known-incomplete
//! RX-error-decoding gap.

#![no_std]
#![deny(unsafe_code)]

use fugit::HertzU32;
use rp2040_hal::pac;
use rp2040_hal::uart::{DataBits, StopBits, UartConfig as HalUartConfig, UartPeripheral};
use rp2040_hal::Timer;

use stablehal_contract::Nanos;
use stablehal_uart::{TxBusy, UartBus, UartConfig, UartError};

/// Bridges the RP2040's free-running 1 MHz timer to `Nanos`. The timer wraps
/// at ~2^64 us; StableHAL runs are far shorter than that, so this is a
/// non-wrapping monotonic clock for our purposes (matches the contract's
/// requirement that `now` never goes backward).
pub struct Clock<'t> {
    timer: &'t Timer,
}

impl<'t> Clock<'t> {
    pub fn new(timer: &'t Timer) -> Self {
        Clock { timer }
    }

    /// Current time as `Nanos`, for driving `poll(now, ..)` calls.
    pub fn now(&self) -> Nanos {
        Nanos::from_micros(self.timer.get_counter().ticks())
    }
}

/// One in-flight received byte or fault, buffered until `try_read` drains it
/// — mirrors the simulator's single-depth RX assumption (no hidden FIFO
/// beyond what the PL011 hardware itself holds).
struct RxSlot {
    pending: Option<Result<u8, UartError>>,
}

/// Adapts one RP2040 PL011 UART peripheral to [`UartBus`]. Single-depth TX
/// (rejects `try_write` while the shift register / hardware FIFO reports
/// busy) so driver pacing stays observable, exactly as the wire contract
/// requires.
pub struct Rp2040UartBus<U> {
    uart: UartPeripheral<rp2040_hal::uart::Enabled, U, (rp2040_hal::gpio::DynPinId, rp2040_hal::gpio::DynPinId)>,
    tx_in_flight: bool,
    rx: RxSlot,
}

impl<U> Rp2040UartBus<U>
where
    U: rp2040_hal::uart::UartDevice,
{
    /// `peripheral_clock_hz` is the system clock feeding the UART's baud
    /// generator (typically 125 MHz on RP2040 after PLL init).
    pub fn new(
        device: U,
        pins: (rp2040_hal::gpio::DynPinId, rp2040_hal::gpio::DynPinId),
        cfg: UartConfig,
        peripheral_clock_hz: HertzU32,
        resets: &mut pac::RESETS,
    ) -> Self {
        let data_bits = match cfg.data_bits {
            8 => DataBits::Eight,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        };
        let stop_bits = if cfg.stop_bits >= 2 {
            StopBits::Two
        } else {
            StopBits::One
        };
        let parity = match cfg.parity {
            stablehal_uart::Parity::None => None,
            stablehal_uart::Parity::Even => Some(rp2040_hal::uart::Parity::Even),
            stablehal_uart::Parity::Odd => Some(rp2040_hal::uart::Parity::Odd),
        };
        let hal_cfg = HalUartConfig {
            baudrate: HertzU32::from_raw(cfg.baud),
            data_bits,
            stop_bits,
            parity,
        };

        // SAFETY-free path: rp2040-hal's UartPeripheral::new/enable handles
        // pin function muxing internally given DynPin GPIO handles.
        let uart = UartPeripheral::new(device, pins, resets)
            .enable(hal_cfg, peripheral_clock_hz)
            .expect("UART init: baud rate representable at this peripheral clock");

        Rp2040UartBus {
            uart,
            tx_in_flight: false,
            rx: RxSlot { pending: None },
        }
    }

    /// Call every poll tick before touching `UartBus` methods: drains one
    /// hardware RX byte (if any) into the single-depth slot, and clears
    /// `tx_in_flight` once the PL011 reports the shift register empty.
    pub fn service(&mut self) {
        if self.tx_in_flight && self.uart.uart_is_writable() {
            self.tx_in_flight = false;
        }
        if self.rx.pending.is_none() {
            let mut byte = [0u8; 1];
            match self.uart.read_raw(&mut byte) {
                Ok(1) => self.rx.pending = Some(Ok(byte[0])),
                _ => {
                    // Error path: rp2040-hal surfaces framing/parity/overrun
                    // via ReadErrorType on read_full_blocking; the raw
                    // nonblocking path used here reports only "no data yet"
                    // for the no-error case. A production target build
                    // should read the PL011 RSR error bits directly via the
                    // rp2040-hal `read_error` API and map them into
                    // `UartError` here (Framing/Parity/Noise/Overrun) —
                    // deferred until this runs against real hardware.
                }
            }
        }
    }
}

impl<U> UartBus for Rp2040UartBus<U>
where
    U: rp2040_hal::uart::UartDevice,
{
    fn try_write(&mut self, byte: u8) -> Result<(), TxBusy> {
        if self.tx_in_flight {
            return Err(TxBusy);
        }
        match self.uart.write_raw(&[byte]) {
            Ok(_) => {
                self.tx_in_flight = true;
                Ok(())
            }
            Err(_) => Err(TxBusy),
        }
    }

    fn try_read(&mut self) -> Option<Result<u8, UartError>> {
        self.rx.pending.take()
    }
}
