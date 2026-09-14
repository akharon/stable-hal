//! `I2cMaster` over a real Linux `i2c-dev` character device
//! (`/dev/i2c-<bus>`).
//!
//! Same shape and same tradeoff as [`crate::spi::SpidevMaster`]: the
//! `I2C_RDWR` ioctl (`i2c-dev.h`) is an atomic combined-transfer primitive
//! — the kernel drives START/repeated-START/STOP and ACK/NACK handling
//! itself, and userspace gets no visibility into individual byte or ACK
//! edges to timestamp. So [`I2cDevMaster::try_begin`] performs the ioctl
//! synchronously and buffers the outcome; `poll_done` hands it back on the
//! next call. The unmodified `ScriptedMaster` reference driver still sees
//! the two-phase `I2cMaster` protocol it's written against.
//!
//! One real difference from spidev: `i2c-dev` *can* report which fault
//! occurred (`ENXIO`/`EREMOTEIO` on no-ACK) with reasonable fidelity, so
//! `try_begin`'s ioctl failure path maps to [`I2cFault::AddrNack`] rather
//! than panicking — closer to the real fault vocabulary than the SPI port
//! could manage.
//!
//! SHC1 has no I2C record encoding yet (only RTU and IPC are registered),
//! so — like the SPI port — this is functional-only: proves the reference
//! driver moves real bytes over a real bus, not yet a capture source.

use std::ffi::CString;
use std::io;
use std::os::unix::io::RawFd;

use stablehal_i2c::{I2cBusy, I2cFault, I2cMaster, I2cOp};

const I2C_RDWR: u64 = 0x0707;
const I2C_M_RD: u16 = 0x0001;

#[repr(C)]
struct I2cMsg {
    addr: u16,
    flags: u16,
    len: u16,
    buf: *mut u8,
}

#[repr(C)]
struct I2cRdwrIoctlData {
    msgs: *mut I2cMsg,
    nmsgs: u32,
}

enum State {
    Idle,
    Done {
        result: Result<(), I2cFault>,
        read_data: Vec<u8>,
    },
}

/// Adapts one `i2c-dev` character device to [`I2cMaster`]. See module doc
/// for why `try_begin` performs the ioctl synchronously.
pub struct I2cDevMaster {
    fd: RawFd,
    state: State,
}

impl I2cDevMaster {
    /// Opens `path` (e.g. `/dev/i2c-1`).
    pub fn open(path: &str) -> io::Result<Self> {
        let fd = open_raw(path)?;
        Ok(I2cDevMaster {
            fd,
            state: State::Idle,
        })
    }
}

impl I2cMaster for I2cDevMaster {
    fn try_begin(&mut self, op: &I2cOp<'_>) -> Result<(), I2cBusy> {
        if matches!(self.state, State::Done { .. }) {
            return Err(I2cBusy);
        }

        let (addr7, mut msg_buf, flags, len) = match op {
            I2cOp::Write { addr7, bytes, .. } => (*addr7, bytes.to_vec(), 0u16, bytes.len()),
            I2cOp::Read { addr7, len, .. } => (*addr7, vec![0u8; *len as usize], I2C_M_RD, *len as usize),
        };

        let mut msg = I2cMsg {
            addr: addr7 as u16,
            flags,
            len: len as u16,
            buf: msg_buf.as_mut_ptr(),
        };
        let mut data = I2cRdwrIoctlData {
            msgs: &mut msg as *mut I2cMsg,
            nmsgs: 1,
        };

        let ret = unsafe { libc::ioctl(self.fd, I2C_RDWR, &mut data as *mut I2cRdwrIoctlData) };
        self.state = if ret < 0 {
            // ENXIO/EREMOTEIO/ETIMEDOUT from i2c-dev generally mean "no
            // device answered the address" at this granularity — the
            // kernel doesn't distinguish addr-nack from data-nack for a
            // single-message transfer, so both map to AddrNack, the
            // closest fit in the closed I2cFault vocabulary.
            State::Done {
                result: Err(I2cFault::AddrNack),
                read_data: Vec::new(),
            }
        } else {
            State::Done {
                result: Ok(()),
                read_data: msg_buf,
            }
        };
        Ok(())
    }

    fn poll_done(&mut self, buf: &mut [u8]) -> Option<Result<(), I2cFault>> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => None,
            State::Done { result, read_data } => {
                let n = read_data.len().min(buf.len());
                buf[..n].copy_from_slice(&read_data[..n]);
                Some(result)
            }
        }
    }
}

impl Drop for I2cDevMaster {
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
