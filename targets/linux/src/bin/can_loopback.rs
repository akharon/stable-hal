//! Self-loopback smoke test: transmits one frame on a SocketCAN interface
//! and reads it back, using the unmodified `CanController` trait — good for
//! `vcan0` (the kernel's virtual CAN driver loops every transmitted frame
//! back to the local socket, no second node or physical bus required).
//!
//! Usage: `linux-can-loopback <interface>` (e.g. `vcan0`, or a real `can0`
//! wired to a transceiver with another node also listening — on a real
//! bus your own transmitted frame does not loop back to you the way
//! `vcan0` does, so this smoke test's "receive our own frame" assumption
//! is `vcan`-specific).
//!
//! Setup for `vcan0`:
//!   sudo modprobe vcan
//!   sudo ip link add dev vcan0 type vcan
//!   sudo ip link set up vcan0

use std::time::{Duration, Instant};

use stablehal_can::{CanController, CanFrame, CanFrameDataBuf, CanId};
use stablehal_linux::can::SocketCan;

fn main() {
    let ifname = std::env::args().nth(1).expect("usage: linux-can-loopback <interface>");

    let mut bus = SocketCan::open(&ifname).expect("open SocketCAN interface");

    let frame = CanFrame {
        id: CanId::Standard(0x123),
        rtr: false,
        data: CanFrameDataBuf::from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]),
        dlc: 4,
    };

    bus.try_transmit(&frame).expect("transmit frame");
    eprintln!("linux-can-loopback: sent {frame:?}");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(result) = bus.try_receive() {
            match result {
                Ok(rx) => {
                    eprintln!("linux-can-loopback: received {rx:?}");
                    assert_eq!(rx.id, frame.id);
                    assert_eq!(rx.data.as_slice(), frame.data.as_slice());
                    println!("OK");
                    return;
                }
                Err(e) => panic!("CAN receive fault: {e:?}"),
            }
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for loopback frame");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
