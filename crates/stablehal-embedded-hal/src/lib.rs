//! Bridge between StableHAL's contract-aware HAL traits and the Rust
//! [`embedded-hal`] ecosystem.
//!
//! A plain HAL is just a signature — `embedded-hal` doesn't bind either
//! side to timing, ordering, or fault behaviour. StableHAL contracts fill
//! that gap. This crate provides adapters so:
//!
//! * **Drivers written against `embedded-hal` traits** can be exercised
//!   inside `stablehal-sim`'s deterministic simulator (DST), judged by
//!   StableHAL's oracles.
//! * **StableHAL's contract-conforming drivers** can interface with
//!   `embedded-hal`-based peripheral implementations on real hardware.
//!
//! # Architecture
//!
//! The crate wraps `embedded-hal` traits behind StableHAL traits and vice
//! versa. In simulation, the StableHAL side drives the deterministic
//! scheduler; the `embedded-hal` side plugs into real hardware HALs.
//!
//! # Feature flags
//!
//! * `spi` — enables SPI bridge via [`SpiMasterBridge`] (requires
//!   `stablehal-spi`).
//! * `can` — enables CAN bridge via [`CanControllerBridge`] (requires
//!   `stablehal-can`).
//!
//! Both are enabled by default.

#![no_std]
// SAFETY: the CAN bridge constructs `embedded_can::StandardId` and
// `embedded_can::ExtendedId` from StableHAL's `CanId` values, which are
// always in valid ranges (0..=0x7FF / 0..=0x1FFFFFFF). Each unsafe block
// is justified inline with a safety comment.

// ---------------------------------------------------------------------------
// UART bridge: embedded-io Read/Write ↔ stablehal_uart::UartBus
// ---------------------------------------------------------------------------

/// Adapts an [`embedded_io::Write`] + [`embedded_io::Read`] (e.g. a real
/// UART) into a [`stablehal_uart::UartBus`] for use in StableHAL's sim and
/// oracle judgement.
pub struct UartBusBridge<T> {
    inner: T,
    read_buf: [u8; 1],
}

impl<T> UartBusBridge<T> {
    pub fn new(inner: T) -> Self {
        UartBusBridge {
            inner,
            read_buf: [0u8; 1],
        }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: embedded_io::Write + embedded_io::Read> stablehal_uart::UartBus for UartBusBridge<T> {
    fn try_write(&mut self, byte: u8) -> Result<(), stablehal_uart::TxBusy> {
        match embedded_io::Write::write(&mut self.inner, &[byte]) {
            Ok(_) => Ok(()),
            _ => Err(stablehal_uart::TxBusy),
        }
    }

    fn try_read(&mut self) -> Option<Result<u8, stablehal_uart::UartError>> {
        match embedded_io::Read::read(&mut self.inner, &mut self.read_buf) {
            Ok(0) => None,
            Ok(_) => Some(Ok(self.read_buf[0])),
            Err(_) => Some(Err(stablehal_uart::UartError::Noise)), // best-effort mapping
        }
    }
}

// ---------------------------------------------------------------------------
// UART bridge: stablehal_uart::UartBus → embedded-io Read/Write
// ---------------------------------------------------------------------------

/// Adapts a [`stablehal_uart::UartBus`] (e.g. the simulator port) into an
/// [`embedded_io::Read`] + [`embedded_io::Write`] so that `embedded-hal`-based
/// drivers can use it.
pub struct UartBusReversed<T> {
    inner: T,
}

impl<T> UartBusReversed<T> {
    pub fn new(inner: T) -> Self {
        UartBusReversed { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: stablehal_uart::UartBus> embedded_io::ErrorType for UartBusReversed<T> {
    type Error = core::convert::Infallible;
}

impl<T: stablehal_uart::UartBus> embedded_io::Read for UartBusReversed<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        for (i, b) in buf.iter_mut().enumerate() {
            match self.inner.try_read() {
                Some(Ok(byte)) => *b = byte,
                _ => return Ok(i),
            }
        }
        Ok(buf.len())
    }
}

impl<T: stablehal_uart::UartBus> embedded_io::Write for UartBusReversed<T> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        for (i, &b) in buf.iter().enumerate() {
            if self.inner.try_write(b).is_err() {
                return Ok(i);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
// ---------------------------------------------------------------------------
// I2C bridge: embedded_hal::i2c::I2c ↔ stablehal_i2c::I2cMaster
// ---------------------------------------------------------------------------

/// Adapts an [`embedded_hal::i2c::I2c`] (real hardware peripheral) into a
/// [`stablehal_i2c::I2cMaster`] for use in StableHAL's conformance suite.
pub struct I2cMasterBridge<T> {
    inner: T,
    op: Option<stablehal_i2c::I2cOp<'static>>,
    read_buf: [u8; 32],
    read_len: u8,
}

impl<T> I2cMasterBridge<T> {
    pub fn new(inner: T) -> Self {
        I2cMasterBridge {
            inner,
            op: None,
            read_buf: [0u8; 32],
            read_len: 0,
        }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: embedded_hal::i2c::I2c> stablehal_i2c::I2cMaster for I2cMasterBridge<T>
where
    <T as embedded_hal::i2c::ErrorType>::Error: core::fmt::Debug,
{
    fn try_begin(&mut self, op: &stablehal_i2c::I2cOp<'_>) -> Result<(), stablehal_i2c::I2cBusy> {
        if self.op.is_some() {
            return Err(stablehal_i2c::I2cBusy);
        }
        match *op {
            stablehal_i2c::I2cOp::Write {
                addr7,
                bytes,
                stop: _,
            } => {
                match embedded_hal::i2c::I2c::write(&mut self.inner, addr7, bytes) {
                    Ok(()) => {}
                    Err(_err) => {
                        // Treat any write error as AddrNack for the polling model.
                        self.op = Some(stablehal_i2c::I2cOp::Write {
                            addr7,
                            bytes: &[],
                            stop: true,
                        });
                    }
                }
            }
            stablehal_i2c::I2cOp::Read {
                addr7,
                len,
                stop: _,
            } => {
                let mut buf = [0u8; 32];
                let read_len = (len as usize).min(32);
                match embedded_hal::i2c::I2c::read(&mut self.inner, addr7, &mut buf[..read_len]) {
                    Ok(()) => {
                        self.read_buf[..read_len].copy_from_slice(&buf[..read_len]);
                        self.read_len = len;
                    }
                    Err(_) => {
                        self.op = Some(stablehal_i2c::I2cOp::Read {
                            addr7,
                            len,
                            stop: true,
                        });
                        return Ok(());
                    }
                }
            }
        }
        self.op = None; // completed synchronously
        Ok(())
    }

    fn poll_done(&mut self, buf: &mut [u8]) -> Option<Result<(), stablehal_i2c::I2cFault>> {
        // The blocking embedded_hal I2c API completes synchronously, so
        // poll_done either returns immediately if no op was pending, or
        // reports a fault.
        if let Some(op) = self.op.take() {
            match op {
                stablehal_i2c::I2cOp::Write { .. } => Some(Err(stablehal_i2c::I2cFault::AddrNack)),
                stablehal_i2c::I2cOp::Read { .. } => Some(Err(stablehal_i2c::I2cFault::AddrNack)),
            }
        } else if self.read_len > 0 {
            let n = (self.read_len as usize).min(buf.len());
            buf[..n].copy_from_slice(&self.read_buf[..n]);
            self.read_len = 0;
            Some(Ok(()))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// I2C slave bridge: embedded_hal::i2c::I2c → stablehal_i2c::I2cSlaveDevice
// ---------------------------------------------------------------------------

/// Wraps an `embedded_hal` I2C peripheral implementation as a StableHAL
/// slave device model, for simulation-side usage.
pub struct I2cSlaveBridge<T> {
    inner: T,
    #[allow(dead_code)]
    addr7: u8,
}

impl<T> I2cSlaveBridge<T> {
    pub fn new(inner: T, addr7: u8) -> Self {
        I2cSlaveBridge { inner, addr7 }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

// The I2cSlaveDevice trait requires byte-level callbacks; embedded_hal
// doesn't expose that level of granularity (its I2c trait is frame-level).
// This adapter provides a best-effort mapping for devices whose embedded_hal
// I2c implementation can be driven one byte at a time.
// For full contract checking, prefer native StableHAL device models.

// ---------------------------------------------------------------------------
// SPI bridge: embedded_hal::spi::SpiDevice ↔ stablehal_spi::SpiMaster
// ---------------------------------------------------------------------------

#[cfg(feature = "spi")]
pub mod spi_bridge {
    use stablehal_spi::{SpiBusy, SpiMaster, SpiMode};

    /// Adapts an [`embedded_hal::spi::SpiDevice`] (a real SPI peripheral)
    /// into a [`stablehal_spi::SpiMaster`] for use in StableHAL's sim/oracle.
    pub struct SpiMasterBridge<T> {
        inner: T,
        pending_result: Option<Result<(), stablehal_spi::SpiModeFault>>,
    }

    impl<T> SpiMasterBridge<T> {
        pub fn new(inner: T) -> Self {
            SpiMasterBridge {
                inner,
                pending_result: None,
            }
        }

        pub fn into_inner(self) -> T {
            self.inner
        }
    }

    impl<T: embedded_hal::spi::SpiDevice<u8>> SpiMaster for SpiMasterBridge<T> {
        fn try_transfer(&mut self, _chip: u8, buf: &mut [u8]) -> Result<(), SpiBusy> {
            // embedded_hal::spi::SpiDevice::transfer is blocking.
            match embedded_hal::spi::SpiDevice::transfer_in_place(&mut self.inner, buf) {
                Ok(()) => {
                    self.pending_result = Some(Ok(()));
                    Ok(())
                }
                Err(_) => {
                    self.pending_result = Some(Err(stablehal_spi::SpiModeFault));
                    Ok(())
                }
            }
        }

        fn poll_done(&mut self) -> Option<Result<(), stablehal_spi::SpiModeFault>> {
            self.pending_result.take()
        }
    }

    /// Adapts a [`stablehal_spi::SpiSlave`] device model into an
    /// `embedded_hal` SPI word.
    pub struct SpiSlaveBridge<T> {
        inner: T,
        mode: Option<SpiMode>,
    }

    impl<T: stablehal_spi::SpiSlave> SpiSlaveBridge<T> {
        pub fn new(inner: T) -> Self {
            SpiSlaveBridge { inner, mode: None }
        }

        pub fn into_inner(self) -> T {
            self.inner
        }
    }

    impl<T: stablehal_spi::SpiSlave> embedded_hal::spi::SpiDevice<u8> for SpiSlaveBridge<T> {
        fn transfer(&mut self, rx: &mut [u8], tx: &[u8]) -> Result<(), Self::Error> {
            let mode = self.mode.unwrap_or(SpiMode::Mode0);
            self.inner.on_select(mode);
            for (i, &byte) in tx.iter().enumerate() {
                let resp = self.inner.xfer_byte(byte);
                if i < rx.len() {
                    rx[i] = resp;
                }
            }
            self.inner.on_deselect();
            Ok(())
        }

        fn transfer_in_place(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
            let mode = self.mode.unwrap_or(SpiMode::Mode0);
            self.inner.on_select(mode);
            for byte in buf.iter_mut() {
                *byte = self.inner.xfer_byte(*byte);
            }
            self.inner.on_deselect();
            Ok(())
        }

        fn transaction(
            &mut self,
            operations: &mut [embedded_hal::spi::Operation<'_, u8>],
        ) -> Result<(), Self::Error> {
            let mode = self.mode.unwrap_or(SpiMode::Mode0);
            self.inner.on_select(mode);
            for op in operations.iter_mut() {
                match op {
                    embedded_hal::spi::Operation::Read(buf) => {
                        for byte in buf.iter_mut() {
                            *byte = self.inner.xfer_byte(0);
                        }
                    }
                    embedded_hal::spi::Operation::Write(buf) => {
                        for &byte in buf.iter() {
                            self.inner.xfer_byte(byte);
                        }
                    }
                    embedded_hal::spi::Operation::Transfer(buf_rx, buf_tx) => {
                        for (rx, &tx) in buf_rx.iter_mut().zip(buf_tx.iter()) {
                            *rx = self.inner.xfer_byte(tx);
                        }
                    }
                    embedded_hal::spi::Operation::TransferInPlace(buf) => {
                        for byte in buf.iter_mut() {
                            *byte = self.inner.xfer_byte(*byte);
                        }
                    }
                    embedded_hal::spi::Operation::DelayNs(_) => {
                        // No-op: StableHAL uses virtual time, not real delays.
                    }
                }
            }
            self.inner.on_deselect();
            Ok(())
        }
    }

    // Map error types.
    impl<T: stablehal_spi::SpiSlave> embedded_hal::spi::ErrorType for SpiSlaveBridge<T> {
        type Error = core::convert::Infallible;
    }
}

// ---------------------------------------------------------------------------
// CAN bridge: embedded-can ↔ stablehal_can::CanController
// ---------------------------------------------------------------------------

#[cfg(feature = "can")]
pub mod can_bridge {
    use stablehal_can::{
        CanController, CanControllerState, CanError, CanFrame, CanFrameDataBuf, CanId,
    };

    /// Adapts an [`embedded_can::blocking::Can`] (a real CAN controller)
    /// into a [`stablehal_can::CanController`] for StableHAL conformance.
    pub struct CanControllerBridge<T> {
        inner: T,
    }

    impl<T> CanControllerBridge<T> {
        pub fn new(inner: T) -> Self {
            CanControllerBridge { inner }
        }

        pub fn into_inner(self) -> T {
            self.inner
        }
    }

    fn data_buf_from_slice(slice: &[u8]) -> CanFrameDataBuf {
        CanFrameDataBuf::from_slice(slice)
    }

    /// Convert an `embedded_can::Id` to a `stablehal_can::CanId`.
    fn map_id(eid: embedded_can::Id) -> CanId {
        match eid {
            embedded_can::Id::Standard(sid) => CanId::Standard(sid.as_raw()),
            embedded_can::Id::Extended(eid_val) => CanId::Extended(eid_val.as_raw()),
        }
    }

    impl<T> CanController for CanControllerBridge<T>
    where
        T: embedded_can::blocking::Can,
        <T as embedded_can::blocking::Can>::Frame: embedded_can::Frame,
    {
        fn try_transmit(&mut self, frame: &CanFrame) -> Result<(), ()> {
            let eid = match frame.id {
                CanId::Standard(raw) => embedded_can::Id::Standard(
                    // SAFETY: validated by CanId constructor.
                    unsafe { embedded_can::StandardId::new_unchecked(raw) },
                ),
                CanId::Extended(raw) => embedded_can::Id::Extended(
                    // SAFETY: validated by CanId constructor.
                    unsafe { embedded_can::ExtendedId::new_unchecked(raw) },
                ),
            };
            let data = frame.data.as_slice();
            let f = match <<T as embedded_can::blocking::Can>::Frame as embedded_can::Frame>::new(
                eid, data,
            ) {
                Some(f) => f,
                None => return Err(()),
            };
            T::transmit(&mut self.inner, &f).map_err(|_| ())
        }

        fn try_receive(&mut self) -> Option<Result<CanFrame, CanError>> {
            use embedded_can::Frame;
            T::receive(&mut self.inner).ok().map(|f| {
                Ok(CanFrame {
                    id: map_id(f.id()),
                    rtr: f.is_remote_frame(),
                    data: data_buf_from_slice(f.data()),
                    dlc: f.dlc() as u8,
                })
            })
        }

        fn state(&self) -> CanControllerState {
            CanControllerState::Active
        }
    }
}
