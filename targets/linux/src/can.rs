//! `CanController` over a real Linux SocketCAN interface (`can0`, `vcan0`,
//! ...) via `AF_CAN`/`SOCK_RAW`/`CAN_RAW`.
//!
//! Unlike the SPI/I2C ports, SocketCAN's raw socket is a genuine
//! non-blocking read/write primitive — one `struct can_frame` per
//! `write()`/`read()`, no atomic combined-transfer ioctl standing between
//! userspace and the wire — so [`SocketCan::try_transmit`]/`try_receive`
//! map directly onto [`CanController`] with no synchronous-ioctl fakery,
//! the same shape as the UART port's real per-byte non-blocking I/O.
//!
//! Also unlike the other Linux ports, SocketCAN ships a real *virtual*
//! interface (`vcan`, the kernel's CAN loopback driver) — so this one can
//! be exercised end-to-end without any physical CAN transceiver:
//!
//! ```sh
//! sudo modprobe vcan
//! sudo ip link add dev vcan0 type vcan
//! sudo ip link set up vcan0
//! ```

use std::ffi::CString;
use std::io;
use std::mem;
use std::os::unix::io::RawFd;

use stablehal_can::{CanControllerState, CanController, CanError, CanFrame, CanFrameDataBuf, CanId};

const AF_CAN: i32 = 29;
const PF_CAN: i32 = AF_CAN;
const CAN_RAW: i32 = 1;
const SOL_CAN_RAW: i32 = 101;
const CAN_RAW_RECV_OWN_MSGS: i32 = 4;
const CAN_EFF_FLAG: u32 = 0x8000_0000;
const CAN_RTR_FLAG: u32 = 0x4000_0000;
const CAN_ERR_FLAG: u32 = 0x2000_0000;
const CAN_SFF_MASK: u32 = 0x0000_07FF;
const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;

#[repr(C)]
struct SockaddrCan {
    can_family: libc::sa_family_t,
    can_ifindex: i32,
    // rx_filter/tx_id union, unused for the default (no filtering) socket
    can_addr: [u8; 8],
}

#[repr(C)]
struct Ifreq {
    ifr_name: [libc::c_char; libc::IFNAMSIZ],
    ifr_ifindex: i32,
    _pad: [u8; 16],
}

/// Wire layout of `struct can_frame` (linux/can.h): 16 bytes.
#[repr(C)]
#[derive(Copy, Clone)]
struct RawCanFrame {
    can_id: u32,
    can_dlc: u8,
    _pad: [u8; 3],
    data: [u8; 8],
}

/// Adapts one SocketCAN raw socket to [`CanController`].
pub struct SocketCan {
    fd: RawFd,
    state: CanControllerState,
}

impl SocketCan {
    /// Opens and binds a `CAN_RAW` socket to `ifname` (e.g. `"vcan0"`,
    /// `"can0"`). Set non-blocking so `try_transmit`/`try_receive` never
    /// stall the poll loop, matching every other `UartBus`/`SpiMaster`-style
    /// port in this project.
    ///
    /// `recv_own_msgs` sets `CAN_RAW_RECV_OWN_MSGS`, off by default in the
    /// kernel: a CAN_RAW socket normally does **not** see frames it just
    /// transmitted, even on `vcan` (this is correct real-bus behavior — a
    /// real transceiver doesn't hear its own TX as a separate RX event
    /// either). Pass `true` only for a single-socket self-test like
    /// `linux-can-loopback`; a real two-node setup (tester socket + DUT
    /// socket, each on their own fd) should leave this `false`.
    pub fn open(ifname: &str, recv_own_msgs: bool) -> io::Result<Self> {
        let fd = unsafe { libc::socket(PF_CAN, libc::SOCK_RAW, CAN_RAW) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        if recv_own_msgs {
            let enable: i32 = 1;
            let ret = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_CAN_RAW,
                    CAN_RAW_RECV_OWN_MSGS,
                    &enable as *const i32 as *const _,
                    mem::size_of::<i32>() as u32,
                )
            };
            if ret < 0 {
                let err = io::Error::last_os_error();
                unsafe { libc::close(fd) };
                return Err(err);
            }
        }

        let ifindex = interface_index(fd, ifname)?;

        let addr = SockaddrCan {
            can_family: AF_CAN as libc::sa_family_t,
            can_ifindex: ifindex,
            can_addr: [0u8; 8],
        };
        let ret = unsafe {
            libc::bind(
                fd,
                &addr as *const SockaddrCan as *const libc::sockaddr,
                mem::size_of::<SockaddrCan>() as u32,
            )
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err);
        }

        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };

        Ok(SocketCan {
            fd,
            state: CanControllerState::Active,
        })
    }
}

impl CanController for SocketCan {
    fn try_transmit(&mut self, frame: &CanFrame) -> Result<(), ()> {
        let mut can_id = frame.id.as_raw();
        can_id |= if matches!(frame.id, CanId::Extended(_)) {
            CAN_EFF_FLAG
        } else {
            0
        };
        if frame.rtr {
            can_id |= CAN_RTR_FLAG;
        }

        let mut data = [0u8; 8];
        let src = frame.data.as_slice();
        data[..src.len()].copy_from_slice(src);

        let raw = RawCanFrame {
            can_id,
            can_dlc: frame.dlc,
            _pad: [0; 3],
            data,
        };

        let n = unsafe {
            libc::write(
                self.fd,
                &raw as *const RawCanFrame as *const _,
                mem::size_of::<RawCanFrame>(),
            )
        };
        if n == mem::size_of::<RawCanFrame>() as isize {
            Ok(())
        } else {
            Err(())
        }
    }

    fn try_receive(&mut self) -> Option<Result<CanFrame, CanError>> {
        let mut raw: RawCanFrame = unsafe { mem::zeroed() };
        let n = unsafe {
            libc::read(
                self.fd,
                &mut raw as *mut RawCanFrame as *mut _,
                mem::size_of::<RawCanFrame>(),
            )
        };
        if n != mem::size_of::<RawCanFrame>() as isize {
            return None; // EWOULDBLOCK/EAGAIN: nothing available yet
        }

        if raw.can_id & CAN_ERR_FLAG != 0 {
            // Kernel error frame: closest fit in the closed CanError
            // vocabulary is BitError (SocketCAN doesn't expose enough
            // detail through this minimal path to distinguish stuff/form/
            // ack/CRC without decoding the error-frame data bytes per
            // linux/can/error.h, which this port doesn't do).
            return Some(Err(CanError::BitError));
        }

        let extended = raw.can_id & CAN_EFF_FLAG != 0;
        let rtr = raw.can_id & CAN_RTR_FLAG != 0;
        let id = if extended {
            CanId::Extended(raw.can_id & CAN_EFF_MASK)
        } else {
            CanId::Standard((raw.can_id & CAN_SFF_MASK) as u16)
        };
        let dlc = raw.can_dlc.min(8);
        let data = CanFrameDataBuf::from_slice(&raw.data[..dlc as usize]);

        Some(Ok(CanFrame {
            id,
            rtr,
            data,
            dlc,
        }))
    }

    fn state(&self) -> CanControllerState {
        self.state
    }
}

impl Drop for SocketCan {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

fn interface_index(fd: RawFd, ifname: &str) -> io::Result<i32> {
    let c_name = CString::new(ifname).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let bytes = c_name.as_bytes_with_nul();
    if bytes.len() > libc::IFNAMSIZ {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "interface name too long"));
    }

    let mut ifr: Ifreq = unsafe { mem::zeroed() };
    for (i, &b) in bytes.iter().enumerate() {
        ifr.ifr_name[i] = b as libc::c_char;
    }

    const SIOCGIFINDEX: u64 = 0x8933;
    let ret = unsafe { libc::ioctl(fd, SIOCGIFINDEX, &mut ifr as *mut Ifreq) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ifr.ifr_ifindex)
}
