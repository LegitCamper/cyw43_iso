//! Host accounting for the dedicated controller ISO packet buffer pool.
//!
//! Bluetooth Core Vol 4, Part E, sections 4.1.1, 7.7.19 and 7.8.2.
//! Credits count HCI packets, including fragments, rather than SDUs. Callers
//! serialize transport writes and accounting, route ISO completion counts here,
//! and discard this state on controller reset. This does not enable ISO support.

use alloc::collections::BTreeMap;
use core::fmt;

use crate::iso::{Direction, IsoError, IsoPacket, MAX_HANDLE};

/// Invalid buffer negotiation, packet, or credit update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditError {
    /// LE Read Buffer Size V2 return parameters have the wrong length.
    ResponseLength,
    /// Controller returned a nonzero HCI status.
    ControllerStatus(u8),
    /// Controller has no dedicated ISO buffers, or its size exceeds 14 bits.
    UnsupportedBuffers,
    /// Packet framing is invalid for host transmission.
    Packet(IsoError),
    /// Packet exceeds the controller's negotiated data length.
    PacketTooLarge,
    /// All controller ISO buffers are occupied.
    Exhausted,
    /// Completion count exceeds packets outstanding for this handle.
    CompletionCount,
}

impl fmt::Display for CreditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResponseLength => f.write_str("invalid LE Read Buffer Size V2 response length"),
            Self::ControllerStatus(status) => write!(f, "HCI status 0x{status:02x}"),
            Self::UnsupportedBuffers => f.write_str("no usable dedicated ISO buffers"),
            Self::Packet(error) => error.fmt(f),
            Self::PacketTooLarge => f.write_str("ISO packet exceeds controller buffer size"),
            Self::Exhausted => f.write_str("controller ISO buffer credits exhausted"),
            Self::CompletionCount => f.write_str("ISO completion exceeds outstanding packets"),
        }
    }
}

impl core::error::Error for CreditError {}

/// One shared ISO transmit pool, with outstanding counts per CIS/BIS handle.
#[derive(Debug)]
pub struct IsoCredits {
    packet_length: u16,
    available: u8,
    outstanding: BTreeMap<u16, u8>,
}

impl IsoCredits {
    /// Uses return parameters of successful LE Read Buffer Size V2 (opcode
    /// `0x2060`), starting with Status, without the Command Complete header.
    ///
    /// # Errors
    /// Rejects failed commands, malformed responses and unavailable ISO buffers.
    pub fn from_buffer_size_v2(parameters: &[u8]) -> Result<Self, CreditError> {
        let status = *parameters.first().ok_or(CreditError::ResponseLength)?;
        if status != 0 {
            return Err(CreditError::ControllerStatus(status));
        }
        if parameters.len() != 7 {
            return Err(CreditError::ResponseLength);
        }
        let packet_length = u16::from_le_bytes([parameters[4], parameters[5]]);
        let available = parameters[6];
        if packet_length == 0 || packet_length > 0x3fff || available == 0 {
            return Err(CreditError::UnsupportedBuffers);
        }
        Ok(Self {
            packet_length,
            available,
            outstanding: BTreeMap::new(),
        })
    }

    /// Controller ISO data length, excluding the four-byte HCI packet header.
    #[must_use]
    pub const fn packet_length(&self) -> u16 {
        self.packet_length
    }

    /// Number of HCI ISO packets that may currently be queued.
    #[must_use]
    pub const fn available(&self) -> u8 {
        self.available
    }

    /// Reserves one packet immediately before submitting it to transport.
    /// If transport definitely sent no bytes, undo with `complete(handle, 1)`.
    /// An ambiguous/partial write requires transport recovery, not a credit refund.
    ///
    /// # Errors
    /// Rejects invalid/oversized packets or exhausted credits without mutation.
    pub fn reserve(&mut self, packet: &IsoPacket<'_>) -> Result<(), CreditError> {
        packet
            .validate(Direction::HostToController)
            .map_err(CreditError::Packet)?;
        let data_length = packet.payload.len()
            + packet
                .sdu
                .map_or(0, |header| 4 + usize::from(header.timestamp.is_some()) * 4);
        if data_length > usize::from(self.packet_length) {
            return Err(CreditError::PacketTooLarge);
        }
        if self.available == 0 {
            return Err(CreditError::Exhausted);
        }
        *self.outstanding.entry(packet.handle).or_default() += 1;
        self.available -= 1;
        Ok(())
    }

    /// Returns credits from one ISO entry of Number Of Completed Packets.
    /// Caller must route ACL entries elsewhere and validate the event envelope.
    /// Completion means the controller released buffers, not peer delivery.
    ///
    /// # Errors
    /// Rejects invalid handles or excess counts without changing accounting.
    pub fn complete(&mut self, handle: u16, count: u16) -> Result<(), CreditError> {
        if handle > MAX_HANDLE {
            return Err(CreditError::Packet(IsoError::Handle));
        }
        let pending = self.outstanding.get(&handle).copied().unwrap_or(0);
        if count > u16::from(pending) {
            return Err(CreditError::CompletionCount);
        }
        let remaining = pending - u8::try_from(count).map_err(|_| CreditError::CompletionCount)?;
        if remaining == 0 {
            self.outstanding.remove(&handle);
        } else {
            self.outstanding.insert(handle, remaining);
        }
        self.available += pending - remaining;
        Ok(())
    }

    /// Reclaims remaining credits after confirmed CIS disconnection or BIG
    /// termination. Never call when merely requesting teardown. Process events
    /// in order and retire old handle state before reusing a controller handle.
    pub fn disconnected(&mut self, handle: u16) {
        if let Some(pending) = self.outstanding.remove(&handle) {
            self.available += pending;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso::{Boundary, PacketStatus, SduHeader};

    fn packet(handle: u16) -> IsoPacket<'static> {
        IsoPacket {
            handle,
            boundary: Boundary::Complete,
            sdu: Some(SduHeader {
                timestamp: None,
                sequence_number: 0,
                length: 2,
                status: PacketStatus::Valid,
            }),
            payload: &[1, 2],
        }
    }

    #[test]
    fn shared_pool_refuses_overcommit_and_recovers_on_completion_and_disconnect() {
        let mut pool = IsoCredits::from_buffer_size_v2(&[0, 0xfb, 0, 4, 6, 0, 2]).unwrap();
        pool.reserve(&packet(1)).unwrap();
        pool.reserve(&packet(2)).unwrap();
        assert_eq!(pool.reserve(&packet(1)), Err(CreditError::Exhausted));
        assert_eq!(pool.complete(1, 2), Err(CreditError::CompletionCount));
        assert_eq!(pool.complete(3, 1), Err(CreditError::CompletionCount));
        assert_eq!(pool.available(), 0);
        pool.complete(1, 1).unwrap();
        pool.disconnected(2);
        pool.disconnected(2);
        assert_eq!(pool.available(), 2);
        pool.reserve(&packet(2)).unwrap();
        assert_eq!(pool.available(), 1);
    }

    #[test]
    fn negotiation_and_packet_failures_leave_credits_untouched() {
        assert!(matches!(
            IsoCredits::from_buffer_size_v2(&[1]),
            Err(CreditError::ControllerStatus(1))
        ));
        for response in [
            &[][..],
            &[0, 0, 0, 0, 6, 0],
            &[0, 0, 0, 0, 0, 0, 2],
            &[0, 0, 0, 0, 6, 0, 0],
            &[0, 0, 0, 0, 0, 0x40, 2],
        ] {
            assert!(IsoCredits::from_buffer_size_v2(response).is_err());
        }
        let mut pool = IsoCredits::from_buffer_size_v2(&[0, 0, 0, 0, 6, 0, 2]).unwrap();
        let mut timestamped = packet(1);
        timestamped.sdu.as_mut().unwrap().timestamp = Some(7);
        assert_eq!(pool.reserve(&timestamped), Err(CreditError::PacketTooLarge));
        assert_eq!(
            pool.reserve(&packet(0xf00)),
            Err(CreditError::Packet(IsoError::Handle))
        );
        assert_eq!(pool.available(), 2);
    }
}
