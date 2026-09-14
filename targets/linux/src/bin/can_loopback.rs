//! Self-loopback smoke test: transmits one frame on a SocketCAN interface
//! and reads it back on the *same* socket, using the unmodified
//! `CanController` trait — good for `vcan0` (no second node or physical bus
//! required), and opens with `recv_own_msgs = true` (`CAN_RAW_RECV_OWN_MSGS`)
//! so this one socket sees its own transmission — off by default, since a
//! real bus doesn't echo your own TX back to you as a separate RX event
//! either, and a real two-node setup should leave it off (see
//! `SocketCan::open`'s doc comment).
//!
//! Usage: `linux-can-loopback <interface>` (e.g. `vcan0`)
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

    let mut bus = SocketCan::open(&ifname, true).expect("open SocketCAN interface");

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
