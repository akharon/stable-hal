//! Linux target glue: `UartBus` over a real termios serial port, plus a
//! monotonic-clock -> `Nanos` bridge and per-byte SHC1 capture — the host-OS
//! analogue of `targets/rp2040`'s `Rp2040UartBus`.
//!
//! This crate contains *no* protocol logic: `RtuMaster`/`RtuSlave` are the
//! unmodified `no_std` drivers from `stablehal-modbus`, driven here only
//! through the `UartBus` trait — same guarantee as the RP2040 target, one
//! rung closer: run against a real USB-UART loopback (two adapters, TX/RX
//! crossed) today, no board bring-up required.
//!
//! Unlike `Rp2040UartBus`, this port also timestamps its own `TxStart`/
//! `TxEnd`/delivery edges into an SHC1 trace as it drives the wire — the
//! std-side equivalent of `ipc_capture_shim::Capture`, since a Linux
//! process can cheaply call `Instant::now()` per byte where an embedded
//! target could not.
//!
//! # Raw mode
//!
//! Serial ports on Linux default to line-disciplined "cooked" mode (echo,
//! signal chars, newline translation) — useless for a byte-exact wire
//! protocol. [`SerialPort::open`] puts the fd in raw mode via `cfmakeraw`
//! equivalent flag clearing, applies the requested baud/framing, and sets
//! `VMIN=0, VTIME=0` so `read()` never blocks — required for the same
//! non-blocking `try_read` semantics every other `UartBus` impl in this
//! project provides.

use std::ffi::CString;
use std::io;
use std::os::unix::io::RawFd;
use std::time::Instant;

use stablehal_contract::{Nanos, TraceEvent};
use stablehal_uart::{LineEvent, Parity, TxBusy, UartBus, UartConfig, UartError};

/// Bridges `std::time::Instant` to `Nanos`, anchored at construction —
/// mirrors `Rp2040UartBus`'s `Clock` (non-wrapping monotonic time, origin
/// arbitrary as long as it never goes backward within one run).
pub struct Clock {
    origin: Instant,
}

impl Clock {
    pub fn new() -> Self {
        Clock {
            origin: Instant::now(),
        }
    }

    pub fn now(&self) -> Nanos {
        Nanos::from_nanos(self.origin.elapsed().as_nanos() as u64)
    }
}

impl Default for Clock {
    fn default() -> Self {
        Clock::new()
    }
}

/// One in-flight received byte or fault, buffered until `try_read` drains
/// it — mirrors `Rp2040UartBus::RxSlot` and the simulator's single-depth RX
/// assumption.
struct RxSlot {
    pending: Option<Result<u8, UartError>>,
}

/// Adapts one real termios serial device to [`UartBus`], recording every
/// `TxStart`/`TxEnd`/delivery edge into an SHC1-ready trace as it goes.
///
/// Single-depth TX, matching every other `UartBus` impl in this project:
/// `try_write` rejects while a previously written byte's nominal char time
/// has not yet elapsed, so driver pacing stays observable rather than
/// hidden behind the kernel's TTY output buffer.
pub struct SerialPort {
    fd: RawFd,
    cfg: UartConfig,
    clock: Clock,
    tx_in_flight: Option<(u8, Nanos)>,
    rx: RxSlot,
    /// SHC1 `line` tag applied to bytes this process transmits (0 for a
    /// master fixture, 1 for a slave fixture) — the complementary line
    /// tags received bytes, matching the RTU convention that line 0 is
    /// master->slave and line 1 is slave->master traffic on the shared wire.
    tx_line: u8,
    trace: Vec<TraceEvent<(u8, LineEvent)>>,
}

impl SerialPort {
    /// Opens `path` (e.g. `/dev/ttyUSB0`), configures raw mode + `cfg`'s
    /// baud/framing, and returns a ready `UartBus`. `tx_line` tags every
    /// byte *this process transmits* (0 or 1) for SHC1 encoding — pass 0 for
    /// a master fixture, 1 for a slave fixture, matching the RTU convention
    /// `stablehal-shc1::Record::line` already uses (master->slave = 0,
    /// slave->master = 1); received bytes are tagged with the other line.
    pub fn open(path: &str, cfg: UartConfig, tx_line: u8) -> io::Result<Self> {
        let fd = open_raw(path)?;
        configure_termios(fd, &cfg)?;
        Ok(SerialPort {
            fd,
            cfg,
            clock: Clock::new(),
            tx_in_flight: None,
            rx: RxSlot { pending: None },
            tx_line,
            trace: Vec::new(),
        })
    }

    pub fn now(&self) -> Nanos {
        self.clock.now()
    }

    /// Must be called every poll iteration before `UartBus` methods are
    /// used: retires a completed TX (records `TxEnd` once its nominal char
    /// time has elapsed) and drains one received byte from the fd, if any.
    pub fn service(&mut self) {
        let now = self.clock.now();
        let rx_line = 1 - self.tx_line;
        if let Some((byte, started)) = self.tx_in_flight {
            if now.saturating_sub(started) >= self.cfg.char_time() {
                self.tx_in_flight = None;
                self.record(now, self.tx_line, LineEvent::TxEnd { byte });
            }
        }
        if self.rx.pending.is_none() {
            let mut buf = [0u8; 1];
            match unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut _, 1) } {
                1 => {
                    self.rx.pending = Some(Ok(buf[0]));
                    self.record(now, rx_line, LineEvent::Delivered { byte: buf[0] });
                }
                0 => {} // nothing available (VMIN=0, VTIME=0 non-blocking read)
                _ => {
                    let err = io::Error::last_os_error();
                    if err.kind() != io::ErrorKind::WouldBlock {
                        // Real ports occasionally report EIO on a hot-unplug;
                        // surface as Overrun rather than panicking the driver
                        // loop — matches the closed UartError fault vocabulary.
                        self.rx.pending = Some(Err(UartError::Overrun));
                        self.record(now, rx_line, LineEvent::DeliveredCorrupt { error: UartError::Overrun });
                    }
                }
            }
        }
    }

    fn record(&mut self, at: Nanos, line: u8, event: LineEvent) {
        self.trace.push(TraceEvent { at, event: (line, event) });
    }

    /// Take the captured trace, e.g. to hand to `stablehal_shc1::write_stream`.
    pub fn take_trace(&mut self) -> Vec<TraceEvent<(u8, LineEvent)>> {
        core::mem::take(&mut self.trace)
    }
}

impl UartBus for SerialPort {
    fn try_write(&mut self, byte: u8) -> Result<(), TxBusy> {
        if self.tx_in_flight.is_some() {
            return Err(TxBusy);
        }
        let n = unsafe { libc::write(self.fd, &byte as *const u8 as *const _, 1) };
        if n != 1 {
            return Err(TxBusy);
        }
        let now = self.clock.now();
        self.tx_in_flight = Some((byte, now));
        self.record(now, self.tx_line, LineEvent::TxStart { byte });
        Ok(())
    }

    fn try_read(&mut self) -> Option<Result<u8, UartError>> {
        self.rx.pending.take()
    }
}

impl Drop for SerialPort {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

fn open_raw(path: &str) -> io::Result<RawFd> {
    let c_path = CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

fn baud_constant(baud: u32) -> io::Result<libc::speed_t> {
    Ok(match baud {
        1200 => libc::B1200,
        2400 => libc::B2400,
        4800 => libc::B4800,
        9600 => libc::B9600,
        19_200 => libc::B19200,
        38_400 => libc::B38400,
        57_600 => libc::B57600,
        115_200 => libc::B115200,
        230_400 => libc::B230400,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported baud rate {baud} (termios fixed-constant table)"),
            ))
        }
    })
}

/// Puts `fd` in raw mode and applies `cfg`'s baud/data/parity/stop-bit
/// framing — the termios equivalent of `Rp2040UartBus::new`'s PL011
/// register programming.
fn configure_termios(fd: RawFd, cfg: &UartConfig) -> io::Result<()> {
    unsafe {
        let mut tio: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut tio) != 0 {
            return Err(io::Error::last_os_error());
        }

        libc::cfmakeraw(&mut tio);

        let speed = baud_constant(cfg.baud)?;
        libc::cfsetispeed(&mut tio, speed);
        libc::cfsetospeed(&mut tio, speed);

        tio.c_cflag &= !libc::CSIZE;
        tio.c_cflag |= match cfg.data_bits {
            5 => libc::CS5,
            6 => libc::CS6,
            7 => libc::CS7,
            _ => libc::CS8,
        };

        tio.c_cflag &= !(libc::PARENB | libc::PARODD);
        match cfg.parity {
            Parity::None => {}
            Parity::Even => tio.c_cflag |= libc::PARENB,
            Parity::Odd => tio.c_cflag |= libc::PARENB | libc::PARODD,
        }

        if cfg.stop_bits >= 2 {
            tio.c_cflag |= libc::CSTOPB;
        } else {
            tio.c_cflag &= !libc::CSTOPB;
        }

        tio.c_cflag |= libc::CLOCAL | libc::CREAD;

        // Non-blocking single-byte reads: the fd is already O_NONBLOCK, but
        // VMIN/VTIME also govern canonical-adjacent behavior on some
        // drivers, so pin both explicitly.
        tio.c_cc[libc::VMIN] = 0;
        tio.c_cc[libc::VTIME] = 0;

        if libc::tcsetattr(fd, libc::TCSANOW, &tio) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::tcflush(fd, libc::TCIOFLUSH) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
