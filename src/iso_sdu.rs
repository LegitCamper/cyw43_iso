//! Bounded host-side HCI ISO SDU fragmentation and reassembly.
//!
//! Implements Bluetooth Core 6.0, Vol 4, Part E, section 5.4.5 packet boundaries.
//! This is not controller ISOAL: no radio PDU segmentation, scheduling,
//! retransmission, or timing recovery happens here.

use alloc::{collections::BTreeMap, vec::Vec};
use core::{fmt, iter::FusedIterator};

use crate::iso::{
    Boundary, Direction, IsoError, IsoPacket, MAX_HANDLE, MAX_SDU_LENGTH, PacketStatus, SduHeader,
};

/// Invalid transport limits, framing, or fragment order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SduError {
    /// Packet framing failed validation.
    Packet(IsoError),
    /// Packet limit cannot hold its SDU header or exceeds the 14-bit wire limit.
    PacketLimit,
    /// Handle count is zero/out of range or SDU limit exceeds 4095 bytes.
    Limits,
    /// No state slot remains; reset an old handle before admitting one.
    Capacity,
    /// Continuation or last fragment arrived without a first fragment.
    UnexpectedFragment,
    /// Accumulated or advertised length violates the SDU limit/header.
    Length,
}

impl fmt::Display for SduError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Packet(error) => error.fmt(f),
            Self::PacketLimit => f.write_str("invalid HCI ISO data packet limit"),
            Self::Limits => f.write_str("invalid HCI ISO reassembly limits"),
            Self::Capacity => f.write_str("HCI ISO handle capacity exhausted"),
            Self::UnexpectedFragment => f.write_str("HCI ISO fragment has no first fragment"),
            Self::Length => f.write_str("HCI ISO reassembled SDU length mismatch"),
        }
    }
}

impl core::error::Error for SduError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Packet(error) => Some(error),
            _ => None,
        }
    }
}

/// Allocation-free packet iterator borrowing one SDU's payload.
#[derive(Debug)]
pub struct SduFragments<'a> {
    handle: u16,
    header: Option<SduHeader>,
    remaining: Option<&'a [u8]>,
    max_data_length: usize,
}

impl<'a> SduFragments<'a> {
    /// Fragments a complete SDU, checking direction-dependent status rules.
    ///
    /// `max_data_length` bounds `Data_Total_Length`: SDU bytes plus any timestamp,
    /// sequence number, and SDU length/status; excludes the four-byte HCI header
    /// and H4 indicator. Caller supplies the negotiated controller packet limit.
    /// Sequence numbers are caller-owned and unchanged across fragmentation.
    ///
    /// # Errors
    /// Rejects invalid SDU metadata/payload or a packet limit outside the wire
    /// range or too small to contain the SDU header.
    pub fn new(
        handle: u16,
        header: SduHeader,
        payload: &'a [u8],
        max_data_length: usize,
        direction: Direction,
    ) -> Result<Self, SduError> {
        IsoPacket {
            handle,
            boundary: Boundary::Complete,
            sdu: Some(header),
            payload,
        }
        .validate(direction)
        .map_err(SduError::Packet)?;
        let header_length = 4 + usize::from(header.timestamp.is_some()) * 4;
        if !(header_length..=0x3fff).contains(&max_data_length) {
            return Err(SduError::PacketLimit);
        }
        Ok(Self {
            handle,
            header: Some(header),
            remaining: Some(payload),
            max_data_length,
        })
    }
}

impl<'a> Iterator for SduFragments<'a> {
    type Item = IsoPacket<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.remaining?;
        let sdu = self.header.take();
        let overhead = sdu.map_or(0, |header| 4 + usize::from(header.timestamp.is_some()) * 4);
        let count = remaining.len().min(self.max_data_length - overhead);
        let (payload, rest) = remaining.split_at(count);
        let boundary = match (sdu.is_some(), rest.is_empty()) {
            (true, true) => Boundary::Complete,
            (true, false) => Boundary::First,
            (false, true) => Boundary::Last,
            (false, false) => Boundary::Continuation,
        };
        self.remaining = (!rest.is_empty()).then_some(rest);
        Some(IsoPacket {
            handle: self.handle,
            boundary,
            sdu,
            payload,
        })
    }
}

impl FusedIterator for SduFragments<'_> {}

/// Completed SDU or explicit controller lost-SDU notification.
#[derive(Debug, PartialEq, Eq)]
pub struct ReceivedSdu {
    /// CIS/BIS connection handle.
    pub handle: u16,
    /// Original timestamp, sequence, declared length, and controller status.
    pub header: SduHeader,
    /// Actual payload; may be shorter for `PossiblyInvalid`, empty for `Lost`.
    pub payload: Vec<u8>,
    /// Previous observed SDU start's sequence plus one, wrapping at 65536.
    /// `None` on a handle's first SDU. A mismatch signals a discontinuity; it
    /// does not establish how many SDUs were lost or whether input was reordered.
    pub expected_sequence: Option<u16>,
}

/// Effects of accepting one HCI packet.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reassembly {
    /// Complete SDU, if this packet completed one.
    pub completed: Option<ReceivedSdu>,
    /// Partial SDU discarded when this packet starts a replacement SDU.
    pub abandoned: Option<SduHeader>,
}

#[derive(Debug, Default)]
struct HandleState {
    next_sequence: Option<u16>,
    partial: Option<ReceivedSdu>,
}

/// Per-direction, per-handle reassembly bounded by configured handles and SDUs.
///
/// Use separate instances for host-to-controller and controller-to-host traffic.
/// Call [`Self::reset_handle`] on disconnect/reuse; completed handles retain their
/// sequence history and consume a slot. Call [`Self::clear`] on controller reset.
#[derive(Debug)]
pub struct SduReassembler {
    direction: Direction,
    max_handles: usize,
    max_sdu_length: usize,
    handles: BTreeMap<u16, HandleState>,
}

impl SduReassembler {
    /// Creates an empty receiver with explicit allocation bounds.
    ///
    /// # Errors
    /// Rejects zero/out-of-range handle capacity or SDU length above 4095.
    pub fn new(
        direction: Direction,
        max_handles: usize,
        max_sdu_length: usize,
    ) -> Result<Self, SduError> {
        if !(1..=usize::from(MAX_HANDLE) + 1).contains(&max_handles)
            || max_sdu_length > MAX_SDU_LENGTH
        {
            return Err(SduError::Limits);
        }
        Ok(Self {
            direction,
            max_handles,
            max_sdu_length,
            handles: BTreeMap::new(),
        })
    }

    /// Consumes exactly one HCI ISO packet without its H4 indicator.
    ///
    /// New first/complete packets replace a partial SDU and report its header.
    /// Lost notifications remain explicit zero-length SDUs. Sequence mismatch
    /// does not reject data: 16-bit sequence numbers alone cannot distinguish
    /// forward loss, duplication, wraparound, and out-of-order input.
    ///
    /// # Errors
    /// Rejects malformed packets, capacity overflow, orphaned fragments, or
    /// invalid accumulated lengths. Any packet error drops that handle's partial
    /// SDU; if a malformed packet lacks a legal handle, all partial SDUs are
    /// dropped. Sequence history survives errors until explicitly reset.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Reassembly, SduError> {
        let packet = IsoPacket::parse(bytes, self.direction).map_err(|error| {
            let handle = bytes
                .get(..2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) & 0x0fff);
            if let Some(handle) = handle.filter(|handle| *handle <= MAX_HANDLE) {
                if let Some(state) = self.handles.get_mut(&handle) {
                    state.partial = None;
                }
            } else {
                for state in self.handles.values_mut() {
                    state.partial = None;
                }
            }
            SduError::Packet(error)
        })?;
        if let Some(header) = packet.sdu {
            if usize::from(header.length) > self.max_sdu_length {
                if let Some(state) = self.handles.get_mut(&packet.handle) {
                    state.partial = None;
                }
                return Err(SduError::Length);
            }
            if !self.handles.contains_key(&packet.handle) && self.handles.len() == self.max_handles
            {
                return Err(SduError::Capacity);
            }
            let state = self.handles.entry(packet.handle).or_default();
            let abandoned = state.partial.take().map(|sdu| sdu.header);
            let mut payload = Vec::with_capacity(usize::from(header.length));
            payload.extend_from_slice(packet.payload);
            let sdu = ReceivedSdu {
                handle: packet.handle,
                header,
                payload,
                expected_sequence: state.next_sequence,
            };
            state.next_sequence = Some(header.sequence_number.wrapping_add(1));
            if packet.boundary == Boundary::Complete {
                return Ok(Reassembly {
                    completed: Some(sdu),
                    abandoned,
                });
            }
            state.partial = Some(sdu);
            return Ok(Reassembly {
                completed: None,
                abandoned,
            });
        }
        let state = self
            .handles
            .get_mut(&packet.handle)
            .ok_or(SduError::UnexpectedFragment)?;
        let mut sdu = state.partial.take().ok_or(SduError::UnexpectedFragment)?;
        let length = sdu.payload.len() + packet.payload.len();
        if length > usize::from(sdu.header.length)
            || (packet.boundary == Boundary::Last
                && sdu.header.status == PacketStatus::Valid
                && length != usize::from(sdu.header.length))
        {
            return Err(SduError::Length);
        }
        sdu.payload.extend_from_slice(packet.payload);
        if packet.boundary == Boundary::Last {
            Ok(Reassembly {
                completed: Some(sdu),
                abandoned: None,
            })
        } else {
            state.partial = Some(sdu);
            Ok(Reassembly::default())
        }
    }

    /// Releases a handle's partial SDU and sequence history, including its slot.
    pub fn reset_handle(&mut self, handle: u16) {
        self.handles.remove(&handle);
    }

    /// Drops all partial SDUs and sequence history after a controller reset.
    pub fn clear(&mut self) {
        self.handles.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const RX: Direction = Direction::ControllerToHost;
    const TX: Direction = Direction::HostToController;

    fn header(length: u16, sequence_number: u16) -> SduHeader {
        SduHeader {
            timestamp: None,
            sequence_number,
            length,
            status: PacketStatus::Valid,
        }
    }

    fn wire(handle: u16, boundary: Boundary, sdu: Option<SduHeader>, payload: &[u8]) -> Vec<u8> {
        IsoPacket {
            handle,
            boundary,
            sdu,
            payload,
        }
        .encode(RX)
        .unwrap()
    }

    #[test]
    fn fragments_match_wire_vector_and_roundtrip_all_packet_limits() {
        let payload = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        let mut h = header(9, 0xffff);
        h.timestamp = Some(0x1234_5678);
        let packets: Vec<_> = SduFragments::new(0x123, h, &payload, 10, TX)
            .unwrap()
            .collect();
        assert_eq!(
            packets[0].encode(TX).unwrap(),
            [
                0x23, 0x41, 10, 0, 0x78, 0x56, 0x34, 0x12, 0xff, 0xff, 9, 0, 1, 2
            ]
        );
        assert_eq!(
            packets[1].encode(TX).unwrap(),
            [0x23, 0x31, 7, 0, 3, 4, 5, 6, 7, 8, 9]
        );
        for limit in 8..=32 {
            let mut receiver = SduReassembler::new(TX, 1, 9).unwrap();
            let mut completed = None;
            for packet in SduFragments::new(0x123, h, &payload, limit, TX).unwrap() {
                let bytes = packet.encode(TX).unwrap();
                assert!(bytes.len() <= limit + 4);
                let result = receiver.push(&bytes).unwrap();
                if result.completed.is_some() {
                    completed = result.completed;
                }
            }
            assert_eq!(
                completed.unwrap(),
                ReceivedSdu {
                    handle: 0x123,
                    header: h,
                    payload: payload.to_vec(),
                    expected_sequence: None
                }
            );
        }
    }

    #[test]
    fn interleaved_handles_preserve_metadata_and_sequence_wrap() {
        let mut receiver = SduReassembler::new(RX, 2, 10).unwrap();
        receiver
            .push(&wire(1, Boundary::First, Some(header(3, 0xffff)), &[1]))
            .unwrap();
        receiver
            .push(&wire(2, Boundary::First, Some(header(2, 7)), &[4]))
            .unwrap();
        receiver
            .push(&wire(1, Boundary::Continuation, None, &[2]))
            .unwrap();
        assert_eq!(
            receiver
                .push(&wire(2, Boundary::Last, None, &[5]))
                .unwrap()
                .completed
                .unwrap()
                .payload,
            [4, 5]
        );
        assert_eq!(
            receiver
                .push(&wire(1, Boundary::Last, None, &[3]))
                .unwrap()
                .completed
                .unwrap()
                .payload,
            [1, 2, 3]
        );
        let wrapped = receiver
            .push(&wire(1, Boundary::Complete, Some(header(0, 0)), &[]))
            .unwrap()
            .completed
            .unwrap();
        assert_eq!(wrapped.expected_sequence, Some(0));
        let gap = receiver
            .push(&wire(1, Boundary::Complete, Some(header(0, 4)), &[]))
            .unwrap()
            .completed
            .unwrap();
        assert_eq!(gap.expected_sequence, Some(1));
    }

    #[test]
    fn replacement_loss_and_short_invalid_sdu_remain_visible() {
        let mut receiver = SduReassembler::new(RX, 1, 10).unwrap();
        receiver
            .push(&wire(1, Boundary::First, Some(header(3, 0)), &[1]))
            .unwrap();
        let mut lost = header(0, 1);
        lost.status = PacketStatus::Lost;
        let result = receiver
            .push(&wire(1, Boundary::Complete, Some(lost), &[]))
            .unwrap();
        assert_eq!(result.abandoned, Some(header(3, 0)));
        assert_eq!(result.completed.unwrap().header.status, PacketStatus::Lost);
        let mut invalid = header(5, 2);
        invalid.status = PacketStatus::PossiblyInvalid;
        invalid.timestamp = Some(42);
        receiver
            .push(&wire(1, Boundary::First, Some(invalid), &[1]))
            .unwrap();
        let sdu = receiver
            .push(&wire(1, Boundary::Last, None, &[2]))
            .unwrap()
            .completed
            .unwrap();
        assert_eq!(sdu.header, invalid);
        assert_eq!(sdu.payload, [1, 2]);
    }

    #[test]
    fn bad_fragment_discards_partial_and_next_start_recovers() {
        let mut receiver = SduReassembler::new(RX, 1, 10).unwrap();
        for bad in [
            wire(1, Boundary::Last, None, &[]),
            wire(1, Boundary::Continuation, None, &[2, 3]),
        ] {
            receiver
                .push(&wire(1, Boundary::First, Some(header(2, 0)), &[1]))
                .unwrap();
            assert_eq!(receiver.push(&bad), Err(SduError::Length));
            assert_eq!(
                receiver.push(&wire(1, Boundary::Last, None, &[2])),
                Err(SduError::UnexpectedFragment)
            );
        }
        receiver
            .push(&wire(1, Boundary::First, Some(header(2, 1)), &[1]))
            .unwrap();
        assert_eq!(
            receiver.push(&[1, 0x10, 1, 0]),
            Err(SduError::Packet(IsoError::PacketLength))
        );
        assert_eq!(
            receiver.push(&wire(1, Boundary::Last, None, &[2])),
            Err(SduError::UnexpectedFragment)
        );
        assert!(
            receiver
                .push(&wire(1, Boundary::Complete, Some(header(1, 2)), &[3]))
                .unwrap()
                .completed
                .is_some()
        );
    }

    #[test]
    fn limits_direction_and_handle_lifecycle_are_enforced() {
        assert!(SduReassembler::new(RX, 0, 10).is_err());
        assert!(SduReassembler::new(RX, 1, MAX_SDU_LENGTH + 1).is_err());
        assert!(SduFragments::new(0, header(0, 0), &[], 3, TX).is_err());
        assert!(SduFragments::new(0, header(0, 0), &[], 0x4000, TX).is_err());
        let mut lost = header(0, 0);
        lost.status = PacketStatus::Lost;
        assert!(matches!(
            SduFragments::new(0, lost, &[], 4, TX),
            Err(SduError::Packet(IsoError::Status))
        ));
        let mut receiver = SduReassembler::new(TX, 1, 1).unwrap();
        assert_eq!(
            receiver.push(&wire(0, Boundary::Complete, Some(lost), &[])),
            Err(SduError::Packet(IsoError::Status))
        );
        receiver
            .push(&wire(0, Boundary::Complete, Some(header(0, 9)), &[]))
            .unwrap();
        assert_eq!(
            receiver.push(&wire(1, Boundary::Complete, Some(header(0, 0)), &[])),
            Err(SduError::Capacity)
        );
        assert_eq!(
            receiver.push(&wire(0, Boundary::First, Some(header(2, 10)), &[1])),
            Err(SduError::Length)
        );
        receiver.reset_handle(0);
        assert_eq!(
            receiver
                .push(&wire(1, Boundary::Complete, Some(header(0, 4)), &[]))
                .unwrap()
                .completed
                .unwrap()
                .expected_sequence,
            None
        );
        receiver.clear();
        assert!(
            receiver
                .push(&wire(0, Boundary::Complete, Some(header(0, 0)), &[]))
                .is_ok()
        );
    }

    #[test]
    fn empty_and_maximum_sdus_roundtrip() {
        for length in [0, MAX_SDU_LENGTH] {
            let payload = vec![0xa5; length];
            let h = header(u16::try_from(length).unwrap(), 8);
            let mut receiver = SduReassembler::new(TX, 1, MAX_SDU_LENGTH).unwrap();
            let mut fragments = SduFragments::new(MAX_HANDLE, h, &payload, 4, TX).unwrap();
            let mut completed = None;
            for packet in fragments.by_ref() {
                completed = receiver
                    .push(&packet.encode(TX).unwrap())
                    .unwrap()
                    .completed;
            }
            assert_eq!(completed.unwrap().payload, payload);
            assert!(fragments.next().is_none());
        }
    }
}
