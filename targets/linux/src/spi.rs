//! `SpiMaster` over a real Linux `spidev` character device
//! (`/dev/spidev<bus>.<cs>`).
//!
//! # Why this doesn't split into two phases like the trait suggests
//!
//! [`stablehal_spi::SpiMaster::try_transfer`]/`poll_done` is a two-phase,
//! poll-driven API — the same shape as every other bus in this project — so
//! the *unmodified* `ScriptedSpiMaster` reference driver can be pointed at
//! it with no code of its own knowing the difference. But `spidev` exposes
//! only one primitive: `SPI_IOC_MESSAGE`, an atomic ioctl that clocks the
//! whole buffer and has the kernel driver assert/de-assert CS around it —
//! userspace gets no separate "start" and "poll for done", and no visibility
//! into individual CS edges or byte boundaries to timestamp.
//!
//! So `try_transfer` here just performs the ioctl synchronously (blocking
//! until the kernel returns) and buffers the outcome; `poll_done` hands
//! that back on the next call. Behaviorally correct — the reference driver
//! sees exactly the two-phase protocol it expects — but note what's lost:
//! this port cannot observe real CS-assert/CS-deassert timing or per-byte
//! SCLK edges, so it cannot feed `SpiContract`'s `SPI-CS-TIMING`/
//! `SPI-XFER-TIME` clauses from genuine hardware edges the way the UART
//! port's `SerialPort` can. A real CS-timing hardware capture would need
//! either a logic analyzer as an independent observer, or bit-banging CS
//! over GPIO with `spidev`'s `SPI_IOC_WR_MAX_SPEED_HZ`/clock-only mode —
//! out of scope here.
//!
//! SHC1 has no SPI record encoding yet (only RTU and IPC contracts are
//! registered — see `stablehal-shc1`'s contract registry); this port is
//! therefore functional-only (proves the unmodified reference driver
//! transfers real bytes over a real bus) and does not yet produce a
//! capture file. Adding `CONTRACT_SPI_V0_1` + record kinds is a separate,
//! larger piece of work (G5: contract-version-affecting change) than
//! porting the OS glue.

use std::ffi::CString;
use std::io;
use std::os::unix::io::RawFd;

use stablehal_spi::{BitOrder, SpiBusy, SpiConfig, SpiMaster, SpiModeFault};

// ioctl request codes for spidev (linux/spi/spidev.h), computed the same
// way the kernel header's macros do: _IOW/_IOR/_IOC with magic 'k' (0x6b).
const SPI_IOC_MAGIC: u8 = b'k';
const SPI_IOC_NR_MODE: u8 = 1;
const SPI_IOC_NR_BITS_PER_WORD: u8 = 3;
const SPI_IOC_NR_MAX_SPEED_HZ: u8 = 4;

const SPI_CPHA: u8 = 0x01;
const SPI_CPOL: u8 = 0x02;
const SPI_LSB_FIRST: u8 = 0x08;

#[repr(C)]
struct SpiIocTransfer {
    tx_buf: u64,
    rx_buf: u64,
    len: u32,
    speed_hz: u32,
    delay_usecs: u16,
    bits_per_word: u8,
    cs_change: u8,
    tx_nbits: u8,
    rx_nbits: u8,
    word_delay_usecs: u8,
    pad: u8,
}

fn ioc_write(nr: u8, size: usize) -> u64 {
    ioc(1, nr, size)
}
fn ioc_read_write(nr: u8, size: usize) -> u64 {
    ioc(3, nr, size)
}
fn ioc(dir: u64, nr: u8, size: usize) -> u64 {
    const NRBITS: u64 = 8;
    const TYPEBITS: u64 = 8;
    const SIZEBITS: u64 = 14;
    (dir << (NRBITS + TYPEBITS + SIZEBITS))
        | ((SPI_IOC_MAGIC as u64) << NRBITS)
        | (nr as u64)
        | ((size as u64) << (NRBITS + TYPEBITS))
}

enum State {
    Idle,
    Done(Result<(), SpiModeFault>),
}

/// Adapts one `spidev` character device to [`SpiMaster`]. See module doc
/// for why `try_transfer` performs the ioctl synchronously rather than
/// truly splitting start/poll.
pub struct SpidevMaster {
    fd: RawFd,
    speed_hz: u32,
    mode_bits: u8,
    state: State,
}

impl SpidevMaster {
    /// Opens `path` (e.g. `/dev/spidev0.0`) and configures mode/speed from
    /// `cfg`. `cfg.sclk_period` is converted to a target `speed_hz`;
    /// `cfg.cs_idle_min` has no spidev equivalent (kernel owns CS timing —
    /// see module doc) and is accepted but unused.
    pub fn open(path: &str, cfg: SpiConfig) -> io::Result<Self> {
        let fd = open_raw(path)?;
        let mut mode_bits = 0u8;
        if cfg.mode.cpol() {
            mode_bits |= SPI_CPOL;
        }
        if cfg.mode.cpha() {
            mode_bits |= SPI_CPHA;
        }
        if matches!(cfg.bit_order, BitOrder::LsbFirst) {
            mode_bits |= SPI_LSB_FIRST;
        }
        let speed_hz = if cfg.sclk_period.as_nanos() == 0 {
            1_000_000
        } else {
            (1_000_000_000u64 / cfg.sclk_period.as_nanos()) as u32
        };

        unsafe {
            let req = ioc_write(SPI_IOC_NR_MODE, 1);
            if libc::ioctl(fd, req, &mode_bits as *const u8) != 0 {
                return Err(io::Error::last_os_error());
            }
            let bits_per_word: u8 = 8;
            let req = ioc_write(SPI_IOC_NR_BITS_PER_WORD, 1);
            if libc::ioctl(fd, req, &bits_per_word as *const u8) != 0 {
                return Err(io::Error::last_os_error());
            }
            let req = ioc_write(SPI_IOC_NR_MAX_SPEED_HZ, 4);
            if libc::ioctl(fd, req, &speed_hz as *const u32) != 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(SpidevMaster {
            fd,
            speed_hz,
            mode_bits,
            state: State::Idle,
        })
    }
}

impl SpiMaster for SpidevMaster {
    fn try_transfer(&mut self, _chip: u8, buf: &mut [u8]) -> Result<(), SpiBusy> {
        // `_chip` is ignored: spidev's device node already names one fixed
        // bus+CS pair (e.g. /dev/spidev0.0); selecting a different chip
        // means opening a different path, not a runtime parameter.
        let xfer = SpiIocTransfer {
            tx_buf: buf.as_ptr() as u64,
            rx_buf: buf.as_mut_ptr() as u64,
            len: buf.len() as u32,
            speed_hz: self.speed_hz,
            delay_usecs: 0,
            bits_per_word: 8,
            cs_change: 0,
            tx_nbits: 0,
            rx_nbits: 0,
            word_delay_usecs: 0,
            pad: 0,
        };
        let req = ioc_read_write(0, std::mem::size_of::<SpiIocTransfer>());
        let ret = unsafe { libc::ioctl(self.fd, req, &xfer as *const SpiIocTransfer) };
        self.state = State::Done(if ret < 0 {
            // The only failure mode this trait's fault vocabulary can
            // express is a CS mode fault; any other ioctl failure (bad fd,
            // unsupported speed, ENOTTY on a non-spidev node) is a setup
            // error, not a runtime protocol fault, so it panics rather than
            // being laundered into SpiModeFault.
            let err = io::Error::last_os_error();
            panic!("spidev SPI_IOC_MESSAGE failed: {err}");
        } else {
            Ok(())
        });
        let _ = self.mode_bits; // retained for Debug/inspection parity with UartConfig-style ports
        Ok(())
    }

    fn poll_done(&mut self) -> Option<Result<(), SpiModeFault>> {
        match self.state {
            State::Idle => None,
            State::Done(result) => {
                self.state = State::Idle;
                Some(result)
            }
        }
    }
}

impl Drop for SpidevMaster {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

fn open_raw(path: &str) -> io::Result<RawFd> {
    let c_path = CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}
