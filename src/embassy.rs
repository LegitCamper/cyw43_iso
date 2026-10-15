//! ISO adapter for Embassy CYW43 0.7's `bt-hci` 0.8 transport.
//!
//! Stock CYW43439 has no ISO buffers: construction fails until controller
//! firmware supplies them. This adapter does not negotiate CIS/BIS or install
//! radio firmware. One task must own transmit state and route ISO completions.

use bt_hci::{FromHciBytes, WriteHci, transport::Transport};
use core::fmt;

use crate::iso::{Direction, IsoError, IsoPacket};
use crate::iso_credit::{CreditError, IsoCredits};

/// CYW43's 1024-byte packet buffer stores H4 plus four-byte-aligned HCI bytes.
/// The HCI header takes four bytes, leaving 1016 for ISO data and SDU headers.
pub const MAX_DATA_LENGTH: u16 = 1016;

/// Validation, backpressure, or transport failure.
#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// ISO framing is invalid.
    Packet(IsoError),
    /// Controller credit or negotiated size check failed.
    Credits(CreditError),
    /// Packet cannot fit CYW43's aligned SDIO buffer.
    Mtu,
    /// Prior write failed or was cancelled; reset controller before rebuilding state.
    RecoveryRequired,
    /// Transport may have sent bytes; credits remain reserved.
    Transport(E),
    /// Validated wire packet was rejected by the dependency parser.
    Conversion,
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Packet(error) => error.fmt(f),
            Self::Credits(error) => error.fmt(f),
            Self::Mtu => f.write_str("ISO packet exceeds CYW43 aligned SDIO buffer"),
            Self::RecoveryRequired => f.write_str("ISO transport requires controller reset"),
            Self::Transport(error) => error.fmt(f),
            Self::Conversion => f.write_str("bt-hci rejected validated ISO packet"),
        }
    }
}

impl<E: core::error::Error + 'static> core::error::Error for Error<E> {}

/// Credit-aware, serialized packet submission to Embassy's Bluetooth transport.
#[derive(Debug)]
pub struct IsoTx {
    credits: IsoCredits,
    recovery_required: bool,
}

impl IsoTx {
    /// Uses LE Read Buffer Size V2 return parameters, including status byte.
    ///
    /// # Errors
    /// Rejects malformed replies and controllers without usable ISO buffers.
    pub fn new(buffer_size_v2: &[u8]) -> Result<Self, CreditError> {
        Ok(Self {
            credits: IsoCredits::from_buffer_size_v2(buffer_size_v2)?,
            recovery_required: false,
        })
    }

    /// Supply this limit to [`crate::iso_sdu::SduFragments::new`].
    #[must_use]
    pub fn packet_length(&self) -> u16 {
        self.credits.packet_length().min(MAX_DATA_LENGTH)
    }

    /// Available controller packet credits, including individual SDU fragments.
    #[must_use]
    pub const fn available(&self) -> u8 {
        self.credits.available()
    }

    /// True after failed/cancelled writes; creating fresh state requires reset.
    #[must_use]
    pub const fn recovery_required(&self) -> bool {
        self.recovery_required
    }

    /// Submit one fragment. Validate and reserve before crossing transport boundary.
    ///
    /// Dropping this future while `write` is pending retains its credit and marks
    /// the session unusable. Reset controller and discard this state to recover.
    /// Successful queueing does not mean peer delivery. ACL/events must still be
    /// serviced by the caller; this function never consumes inbound packets.
    ///
    /// # Errors
    /// Returns validation, MTU, credit, or transport errors. Transport failure
    /// does not refund credits because a partial/ambiguous write may have occurred.
    pub async fn send<T: Transport>(
        &mut self,
        transport: &T,
        packet: &IsoPacket<'_>,
    ) -> Result<(), Error<T::Error>> {
        if self.recovery_required {
            return Err(Error::RecoveryRequired);
        }
        packet
            .validate(Direction::HostToController)
            .map_err(Error::Packet)?;
        if packet.encoded_len() > usize::from(MAX_DATA_LENGTH) + 4 {
            return Err(Error::Mtu);
        }
        let mut wire = [0; MAX_DATA_LENGTH as usize + 4];
        let length = packet
            .encode_into(Direction::HostToController, &mut wire)
            .map_err(Error::Packet)?;
        let adapted = bt_hci::data::IsoPacket::from_hci_bytes_complete(&wire[..length])
            .map_err(|_| Error::Conversion)?;
        self.credits.reserve(packet).map_err(Error::Credits)?;
        self.recovery_required = true;
        transport.write(&adapted).await.map_err(Error::Transport)?;
        self.recovery_required = false;
        Ok(())
    }

    /// Route one ISO entry from Number Of Completed Packets here; ACL entries
    /// belong to the ACL pool. Validate the event envelope before dispatch.
    ///
    /// # Errors
    /// Rejects excess counts and invalid handles without changing credits.
    pub fn completed(&mut self, handle: u16, count: u16) -> Result<(), CreditError> {
        self.credits.complete(handle, count)
    }

    /// Reclaim credits only after confirmed CIS disconnection or BIG termination.
    pub fn disconnected(&mut self, handle: u16) {
        self.credits.disconnected(handle);
    }
}

/// Revalidate a received `bt-hci` ISO packet, preserving status bits and timestamps.
/// Scratch storage must hold the four-byte HCI header plus its data load.
/// The serialized bytes in scratch can also be passed to
/// [`crate::iso_sdu::SduReassembler::push`] for reassembly.
///
/// # Errors
/// Rejects insufficient storage or malformed controller framing.
pub fn receive<'a>(
    packet: &bt_hci::data::IsoPacket<'_>,
    scratch: &'a mut [u8],
) -> Result<IsoPacket<'a>, IsoError> {
    let length = packet.size();
    let output = scratch.get_mut(..length).ok_or(IsoError::BufferTooSmall)?;
    packet
        .write_hci(&mut *output)
        .map_err(|_| IsoError::BufferTooSmall)?;
    IsoPacket::parse(output, Direction::ControllerToHost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso::{Boundary, PacketStatus, SduHeader};
    use alloc::vec::Vec;
    use bt_hci::{ControllerToHostPacket, HostToControllerPacket, transport::WithIndicator};
    use core::{
        cell::RefCell,
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    #[derive(Default)]
    struct FakeTransport {
        packets: RefCell<Vec<Vec<u8>>>,
        pending: bool,
        fail: bool,
    }

    impl embedded_io::ErrorType for FakeTransport {
        type Error = embedded_io::ErrorKind;
    }

    impl Transport for FakeTransport {
        async fn read<'a>(
            &self,
            _: &'a mut [u8],
        ) -> Result<ControllerToHostPacket<'a>, Self::Error> {
            core::future::pending().await
        }

        async fn write<T: HostToControllerPacket>(&self, packet: &T) -> Result<(), Self::Error> {
            if self.pending {
                core::future::pending::<()>().await;
            }
            if self.fail {
                return Err(embedded_io::ErrorKind::Other);
            }
            let mut bytes = alloc::vec![0; packet.size() + 1];
            WithIndicator::new(packet)
                .write_hci(&mut bytes[..])
                .unwrap();
            self.packets.borrow_mut().push(bytes);
            Ok(())
        }
    }

    fn ready<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("unexpected pending mock operation"),
        }
    }

    fn packet(payload: &[u8]) -> IsoPacket<'_> {
        IsoPacket {
            handle: 1,
            boundary: Boundary::Complete,
            sdu: Some(SduHeader {
                timestamp: Some(0xffff_fff0),
                sequence_number: 9,
                length: u16::try_from(payload.len()).unwrap(),
                status: PacketStatus::Valid,
            }),
            payload,
        }
    }

    #[test]
    fn send_preserves_wire_and_obeys_credit_pool() {
        let mut tx = IsoTx::new(&[0, 0, 0, 0, 20, 0, 1]).unwrap();
        let transport = FakeTransport::default();
        let packet = packet(&[0xaa, 0xbb]);
        ready(tx.send(&transport, &packet)).unwrap();
        assert_eq!(tx.available(), 0);
        assert_eq!(
            ready(tx.send(&transport, &packet)),
            Err(Error::Credits(CreditError::Exhausted))
        );
        let packets = transport.packets.borrow();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0][0], 5);
        assert_eq!(
            IsoPacket::parse(&packets[0][1..], Direction::HostToController).unwrap(),
            packet
        );
        tx.completed(1, 1).unwrap();
        assert_eq!(tx.available(), 1);
    }

    #[test]
    fn cancellation_and_write_failure_require_reset_without_refund() {
        for pending in [true, false] {
            let mut tx = IsoTx::new(&[0, 0, 0, 0, 20, 0, 2]).unwrap();
            let transport = FakeTransport {
                pending,
                fail: !pending,
                ..FakeTransport::default()
            };
            let packet = packet(&[1]);
            {
                let mut future = pin!(tx.send(&transport, &packet));
                let result = future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()));
                if pending {
                    assert!(result.is_pending());
                } else {
                    assert_eq!(
                        result,
                        Poll::Ready(Err(Error::Transport(embedded_io::ErrorKind::Other)))
                    );
                }
            }
            assert!(tx.recovery_required());
            assert_eq!(tx.available(), 1);
            assert_eq!(
                ready(tx.send(&FakeTransport::default(), &packet)),
                Err(Error::RecoveryRequired)
            );
            tx.disconnected(1);
            assert!(tx.recovery_required());
        }
    }

    #[test]
    fn aligned_mtu_and_invalid_packet_fail_before_reserving() {
        let mut tx = IsoTx::new(&[0, 0, 0, 0, 0xff, 0x3f, 2]).unwrap();
        let transport = FakeTransport::default();
        assert_eq!(tx.packet_length(), MAX_DATA_LENGTH);
        ready(tx.send(&transport, &packet(&[0; MAX_DATA_LENGTH as usize - 8]))).unwrap();
        assert_eq!(
            ready(tx.send(&transport, &packet(&[0; MAX_DATA_LENGTH as usize - 7]))),
            Err(Error::Mtu)
        );
        let mut invalid = packet(&[]);
        invalid.handle = 0xffff;
        assert_eq!(
            ready(tx.send(&transport, &invalid)),
            Err(Error::Packet(IsoError::Handle))
        );
        assert_eq!(tx.available(), 1);
    }

    #[test]
    fn receive_preserves_loss_and_rejects_reserved_status() {
        let mut scratch = [0; 16];
        let lost = [1, 0x20, 4, 0, 7, 0, 0, 0x80];
        let packet = bt_hci::data::IsoPacket::from_hci_bytes_complete(&lost).unwrap();
        assert_eq!(
            receive(&packet, &mut scratch).unwrap().sdu.unwrap().status,
            PacketStatus::Lost
        );
        assert_eq!(
            receive(&packet, &mut scratch[..7]),
            Err(IsoError::BufferTooSmall)
        );
        let reserved = [1, 0x20, 4, 0, 7, 0, 0, 0xc0];
        let packet = bt_hci::data::IsoPacket::from_hci_bytes_complete(&reserved).unwrap();
        assert_eq!(receive(&packet, &mut scratch), Err(IsoError::Status));
    }

    #[test]
    fn stock_controller_cannot_create_iso_session() {
        assert!(matches!(
            IsoTx::new(&[1]),
            Err(CreditError::ControllerStatus(1))
        ));
        assert!(matches!(
            IsoTx::new(&[0; 7]),
            Err(CreditError::UnsupportedBuffers)
        ));
    }
}
