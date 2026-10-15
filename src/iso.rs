//! Software-only HCI ISO packet framing, per Bluetooth Core 6.0, Vol 4, Part E,
//! section 5.4.5. This does not enable CYW43 ISO transport or controller support;
//! LC3 encoding, LE Audio profiles, and connection setup are absent. See
//! [`crate::iso_sdu`] for SDU transport and [`crate::iso_credit`] for buffer credits.
//!
//! Packets exclude the H4 packet indicator (`0x05`). Parsing borrows payload bytes
//! and rejects nonzero RFU bits for strict capture validation. Callers must enforce
//! negotiated controller buffer limits before transmission.
//!
//! Specification: <https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-60/out/en/host-controller-interface/host-controller-interface-functional-specification.html>

use alloc::vec::Vec;
use core::fmt;

/// Largest legal connection handle (the wire field is 12 bits).
pub const MAX_HANDLE: u16 = 0x0eff;
/// Largest SDU representable by the 12-bit SDU length field.
pub const MAX_SDU_LENGTH: usize = 0x0fff;

/// Packet direction determines whether status flags are permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Status bits are reserved and must be zero.
    HostToController,
    /// Status describes reception over the isochronous physical channel.
    ControllerToHost,
}

/// HCI ISO packet boundary flag (PB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Boundary {
    /// First fragment, carrying the SDU header.
    First = 0,
    /// Intermediate fragment without an SDU header.
    Continuation = 1,
    /// Complete SDU, carrying the SDU header.
    Complete = 2,
    /// Last fragment without an SDU header.
    Last = 3,
}

/// Controller reception status, stored in bits 14..15 of the SDU length word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PacketStatus {
    /// Complete SDU received correctly; also the required host transmission value.
    Valid = 0,
    /// Payload may have errors or missing SDU data.
    PossiblyInvalid = 1,
    /// Lost SDU notification; no payload and zero SDU length.
    Lost = 2,
}

/// Metadata present only in first and complete packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SduHeader {
    /// Optional timestamp in microseconds, in the controller's clock domain.
    pub timestamp: Option<u32>,
    /// SDU sequence number, wrapping at 65536.
    pub sequence_number: u16,
    /// Total SDU length, not this fragment's length.
    pub length: u16,
    /// Reception status, or `Valid` for host transmission.
    pub status: PacketStatus,
}

/// One borrowed HCI ISO packet. Public fields are checked by [`Self::encode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoPacket<'a> {
    /// Assigned CIS or BIS handle in `0x000..=0xeff`.
    pub handle: u16,
    /// Fragment position within the SDU.
    pub boundary: Boundary,
    /// Present exactly when boundary is `First` or `Complete`.
    pub sdu: Option<SduHeader>,
    /// SDU bytes or fragment bytes; may be empty.
    pub payload: &'a [u8],
}

/// Invalid HCI ISO framing or field combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoError {
    /// Required packet or SDU header bytes are missing.
    Truncated,
    /// Packet size differs from its 14-bit data length.
    PacketLength,
    /// An RFU bit is set.
    ReservedBits,
    /// Handle exceeds `0xeff`.
    Handle,
    /// Timestamp flag is set on a continuation or last fragment.
    Timestamp,
    /// SDU header presence disagrees with the boundary flag.
    SduHeader,
    /// Status is reserved, forbidden in this direction, or violates lost-SDU rules.
    Status,
    /// SDU or fragment length is too large or inconsistent with its header.
    SduLength,
    /// Caller-provided output buffer cannot hold the encoded packet.
    BufferTooSmall,
}

impl fmt::Display for IsoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated HCI ISO header",
            Self::PacketLength => "HCI ISO packet length mismatch",
            Self::ReservedBits => "nonzero HCI ISO reserved bits",
            Self::Handle => "HCI ISO handle exceeds 0xeff",
            Self::Timestamp => "HCI ISO timestamp on continuation or last fragment",
            Self::SduHeader => "HCI ISO SDU header disagrees with boundary flag",
            Self::Status => "invalid HCI ISO packet status",
            Self::SduLength => "invalid HCI ISO SDU or fragment length",
            Self::BufferTooSmall => "HCI ISO output buffer too small",
        })
    }
}

impl core::error::Error for IsoError {}

impl<'a> IsoPacket<'a> {
    /// Parses exactly one packet, excluding its H4 indicator, without allocation.
    ///
    /// # Errors
    /// Returns [`IsoError`] for truncated headers, extra bytes, reserved fields,
    /// invalid lengths, or field combinations forbidden in `direction`.
    pub fn parse(bytes: &'a [u8], direction: Direction) -> Result<Self, IsoError> {
        let header = bytes.get(..4).ok_or(IsoError::Truncated)?;
        let flags = u16::from_le_bytes([header[0], header[1]]);
        let data_length = u16::from_le_bytes([header[2], header[3]]);
        if flags & 0x8000 != 0 || data_length & 0xc000 != 0 {
            return Err(IsoError::ReservedBits);
        }
        if bytes.len() - 4 != usize::from(data_length) {
            return Err(IsoError::PacketLength);
        }
        let boundary = match (flags >> 12) & 3 {
            0 => Boundary::First,
            1 => Boundary::Continuation,
            2 => Boundary::Complete,
            _ => Boundary::Last,
        };
        let has_timestamp = flags & 0x4000 != 0;
        let mut payload = &bytes[4..];
        let sdu = if matches!(boundary, Boundary::First | Boundary::Complete) {
            let timestamp = if has_timestamp {
                let stamp = payload.get(..4).ok_or(IsoError::Truncated)?;
                let value = u32::from_le_bytes([stamp[0], stamp[1], stamp[2], stamp[3]]);
                payload = &payload[4..];
                Some(value)
            } else {
                None
            };
            let header = payload.get(..4).ok_or(IsoError::Truncated)?;
            let length_status = u16::from_le_bytes([header[2], header[3]]);
            if length_status & 0x3000 != 0 {
                return Err(IsoError::ReservedBits);
            }
            let status = match length_status >> 14 {
                0 => PacketStatus::Valid,
                1 => PacketStatus::PossiblyInvalid,
                2 => PacketStatus::Lost,
                _ => return Err(IsoError::Status),
            };
            payload = &payload[4..];
            Some(SduHeader {
                timestamp,
                sequence_number: u16::from_le_bytes([header[0], header[1]]),
                length: length_status & 0x0fff,
                status,
            })
        } else {
            if has_timestamp {
                return Err(IsoError::Timestamp);
            }
            None
        };
        let packet = Self {
            handle: flags & 0x0fff,
            boundary,
            sdu,
            payload,
        };
        packet.validate(direction)?;
        Ok(packet)
    }

    /// Encodes one packet, excluding the H4 indicator, after checking all fields.
    ///
    /// # Errors
    /// Returns [`IsoError`] for invalid handles, lengths, SDU header presence,
    /// or status combinations. Negotiated controller buffer limits are not checked.
    pub fn encode(&self, direction: Direction) -> Result<Vec<u8>, IsoError> {
        self.validate(direction)?;
        let mut bytes = alloc::vec![0; self.encoded_len()];
        self.encode_into(direction, &mut bytes)?;
        Ok(bytes)
    }

    /// Packet size excluding the H4 indicator. Validate before trusting public fields.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        4 + self.payload.len()
            + self
                .sdu
                .map_or(0, |header| 4 + usize::from(header.timestamp.is_some()) * 4)
    }

    /// Encodes into caller-owned storage without allocating; returns bytes used.
    ///
    /// # Errors
    /// Rejects invalid packets or insufficient storage before changing `output`.
    pub fn encode_into(&self, direction: Direction, output: &mut [u8]) -> Result<usize, IsoError> {
        self.validate(direction)?;
        let length = self.encoded_len();
        let bytes = output.get_mut(..length).ok_or(IsoError::BufferTooSmall)?;
        let has_timestamp = self.sdu.is_some_and(|sdu| sdu.timestamp.is_some());
        let data_length = u16::try_from(length - 4).map_err(|_| IsoError::PacketLength)?;
        let flags = self.handle | ((self.boundary as u16) << 12) | (u16::from(has_timestamp) << 14);
        bytes[..2].copy_from_slice(&flags.to_le_bytes());
        bytes[2..4].copy_from_slice(&data_length.to_le_bytes());
        let mut offset = 4;
        if let Some(sdu) = self.sdu {
            if let Some(timestamp) = sdu.timestamp {
                bytes[offset..offset + 4].copy_from_slice(&timestamp.to_le_bytes());
                offset += 4;
            }
            bytes[offset..offset + 2].copy_from_slice(&sdu.sequence_number.to_le_bytes());
            bytes[offset + 2..offset + 4]
                .copy_from_slice(&(sdu.length | ((sdu.status as u16) << 14)).to_le_bytes());
            offset += 4;
        }
        bytes[offset..].copy_from_slice(self.payload);
        Ok(length)
    }

    pub(crate) fn validate(&self, direction: Direction) -> Result<(), IsoError> {
        if self.handle > MAX_HANDLE {
            return Err(IsoError::Handle);
        }
        if self.payload.len() > MAX_SDU_LENGTH {
            return Err(IsoError::SduLength);
        }
        if self.sdu.is_some() != matches!(self.boundary, Boundary::First | Boundary::Complete) {
            return Err(IsoError::SduHeader);
        }
        if let Some(sdu) = self.sdu {
            if direction == Direction::HostToController && sdu.status != PacketStatus::Valid {
                return Err(IsoError::Status);
            }
            if sdu.status == PacketStatus::Lost
                && (self.boundary != Boundary::Complete
                    || sdu.length != 0
                    || !self.payload.is_empty())
            {
                return Err(IsoError::Status);
            }
            if usize::from(sdu.length) > MAX_SDU_LENGTH
                || self.payload.len() > usize::from(sdu.length)
                || (self.boundary == Boundary::Complete
                    && sdu.status == PacketStatus::Valid
                    && self.payload.len() != usize::from(sdu.length))
            {
                return Err(IsoError::SduLength);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const RX: Direction = Direction::ControllerToHost;
    const TX: Direction = Direction::HostToController;

    #[test]
    fn complete_packet_matches_wire_vector() {
        let wire = [0x23, 0x21, 7, 0, 0xfe, 0xff, 3, 0, 0xaa, 0xbb, 0xcc];
        let packet = IsoPacket::parse(&wire, TX).unwrap();
        assert_eq!(packet.handle, 0x123);
        assert_eq!(packet.boundary, Boundary::Complete);
        assert_eq!(packet.sdu.unwrap().sequence_number, 0xfffe);
        assert_eq!(packet.payload, [0xaa, 0xbb, 0xcc]);
        assert_eq!(packet.encode(TX).unwrap(), wire);
    }

    #[test]
    fn first_fragment_with_timestamp_matches_wire_vector() {
        let wire = [
            0xff, 0x4e, 10, 0, 0x78, 0x56, 0x34, 0x12, 0xff, 0xff, 4, 0, 1, 2,
        ];
        let packet = IsoPacket::parse(&wire, TX).unwrap();
        assert_eq!(packet.handle, MAX_HANDLE);
        assert_eq!(packet.boundary, Boundary::First);
        assert_eq!(packet.sdu.unwrap().timestamp, Some(0x1234_5678));
        assert_eq!(packet.sdu.unwrap().length, 4);
        assert_eq!(packet.encode(TX).unwrap(), wire);
    }

    #[test]
    fn all_boundaries_allow_empty_fragments() {
        for wire in [
            vec![0, 0, 4, 0, 0, 0, 1, 0],
            vec![0, 0x10, 0, 0],
            vec![0, 0x20, 4, 0, 0, 0, 0, 0],
            vec![0, 0x30, 0, 0],
        ] {
            let packet = IsoPacket::parse(&wire, TX).unwrap();
            assert_eq!(packet.payload, []);
            assert_eq!(packet.encode(TX).unwrap(), wire);
        }
    }

    #[test]
    fn status_bits_are_high_bits_and_direction_dependent() {
        let possibly_invalid = [0, 0x20, 5, 0, 7, 0, 2, 0x40, 0xaa];
        let packet = IsoPacket::parse(&possibly_invalid, RX).unwrap();
        assert_eq!(packet.sdu.unwrap().status, PacketStatus::PossiblyInvalid);
        assert_eq!(packet.encode(RX).unwrap(), possibly_invalid);
        assert_eq!(packet.encode(TX), Err(IsoError::Status));
        assert_eq!(
            IsoPacket::parse(&possibly_invalid, TX),
            Err(IsoError::Status)
        );
        let lost = [0, 0x20, 4, 0, 8, 0, 0, 0x80];
        assert_eq!(
            IsoPacket::parse(&lost, RX).unwrap().encode(RX).unwrap(),
            lost
        );
    }

    #[test]
    fn rejects_all_reserved_fields_and_timestamp_on_later_fragments() {
        for wire in [
            vec![0, 0xa0, 4, 0, 0, 0, 0, 0],
            vec![0, 0x20, 4, 0x40, 0, 0, 0, 0],
            vec![0, 0x20, 4, 0x80, 0, 0, 0, 0],
            vec![0, 0x20, 4, 0, 0, 0, 0, 0x10],
            vec![0, 0x20, 4, 0, 0, 0, 0, 0x20],
        ] {
            assert_eq!(IsoPacket::parse(&wire, RX), Err(IsoError::ReservedBits));
        }
        for flag in [0x50, 0x70] {
            assert_eq!(
                IsoPacket::parse(&[0, flag, 0, 0], RX),
                Err(IsoError::Timestamp)
            );
        }
        assert_eq!(
            IsoPacket::parse(&[0, 0x20, 4, 0, 0, 0, 0, 0xc0], RX),
            Err(IsoError::Status)
        );
    }

    #[test]
    fn rejects_truncation_extra_bytes_and_invalid_lengths() {
        let wire = [1, 0x60, 9, 0, 1, 2, 3, 4, 5, 6, 1, 0, 0xaa];
        for len in 0..wire.len() {
            assert!(IsoPacket::parse(&wire[..len], RX).is_err());
        }
        let mut extra = wire.to_vec();
        extra.push(0);
        assert_eq!(IsoPacket::parse(&extra, RX), Err(IsoError::PacketLength));
        for wire in [
            vec![0, 0, 3, 0, 0, 0, 0],
            vec![0, 0x60, 7, 0, 0, 0, 0, 0, 0, 0, 0],
            vec![0, 0x20, 4, 0, 0, 0, 1, 0],
            vec![0, 0, 5, 0, 0, 0, 0, 0, 1],
        ] {
            assert!(IsoPacket::parse(&wire, RX).is_err());
        }
    }

    #[test]
    fn rejects_lost_status_with_data_or_fragmentation() {
        for wire in [
            vec![0, 0, 4, 0, 0, 0, 0, 0x80],
            vec![0, 0x20, 4, 0, 0, 0, 1, 0x80],
            vec![0, 0x20, 5, 0, 0, 0, 0, 0x80, 1],
        ] {
            assert_eq!(IsoPacket::parse(&wire, RX), Err(IsoError::Status));
        }
    }

    #[test]
    fn encoder_checks_constructed_packets_and_maximum_sdu() {
        let payload = [0xaa; MAX_SDU_LENGTH];
        let mut packet = IsoPacket {
            handle: MAX_HANDLE,
            boundary: Boundary::Complete,
            sdu: Some(SduHeader {
                timestamp: Some(u32::MAX),
                sequence_number: u16::MAX,
                length: 0x0fff,
                status: PacketStatus::Valid,
            }),
            payload: &payload,
        };
        let wire = packet.encode(TX).unwrap();
        assert_eq!(IsoPacket::parse(&wire, TX).unwrap(), packet);
        packet.handle += 1;
        assert_eq!(packet.encode(TX), Err(IsoError::Handle));
        packet.handle = 0;
        packet.sdu.as_mut().unwrap().length += 1;
        assert_eq!(packet.encode(TX), Err(IsoError::SduLength));
        packet.sdu = None;
        assert_eq!(packet.encode(TX), Err(IsoError::SduHeader));
        packet.boundary = Boundary::Last;
        packet.payload = &[0; MAX_SDU_LENGTH + 1];
        assert_eq!(packet.encode(TX), Err(IsoError::SduLength));
    }

    #[test]
    fn parser_rejects_reserved_handles_and_oversized_fragments() {
        assert_eq!(
            IsoPacket::parse(&[0, 0x1f, 0, 0], RX),
            Err(IsoError::Handle)
        );
        let mut wire = vec![0; 4 + MAX_SDU_LENGTH + 1];
        wire[1] = 0x10;
        wire[3] = 0x10;
        assert_eq!(IsoPacket::parse(&wire, RX), Err(IsoError::SduLength));
    }
}
