//! Framed ISOAL at the ISOAL/Link Layer boundary, with no radio dependency.
//!
//! Bluetooth Core 5.4, Vol 6, Part G, sections 2.2, 3.1, 3.2.1 and 4:
//! <https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/low-energy-controller/isochronous-adaptation-layer.html>
//! Header bit positions also agree with Zephyr v4.2.0 `pdu_iso_sdu_sh`:
//! <https://github.com/zephyrproject-rtos/zephyr/blob/v4.2.0/subsys/bluetooth/controller/ll_sw/pdu.h>
//!
//! The Link Layer supplies original payload numbers, removes retransmissions,
//! LL headers and MIC, and reports integrity failures as missing payloads.
//! Anchors refer to the *original* transmission event, excluding retransmission
//! delays. Call the transmitter BN times per event, including empty padding.
//! Scheduling, BN/bandwidth admission and flush deadlines belong to the caller.

use alloc::vec::Vec;
use core::fmt;

/// LLID identifying framed ISO Data PDUs, including zero-length padding.
pub const FRAMED_LLID: u8 = 2;
const PAYLOAD_MASK: u64 = (1 << 39) - 1;

/// Invalid configuration, input, or framed wire data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramedError {
    /// `Max_PDU`, `Max_SDU`, or `SDU_Interval` is outside supported limits.
    Configuration,
    /// Input SDU exceeds negotiated `Max_SDU`.
    SduLength,
    /// `Time_Offset` is not positive or cannot fit in 24 bits.
    TimeOffset,
    /// SDU timestamps go backwards or are too close to distinguish intervals.
    TimestampOrder,
    /// Payload number is outside 39 bits, duplicated, or out of order.
    PayloadNumber,
    /// Invalid LLID, oversized PDU, or malformed segmentation headers.
    MalformedPdu,
}

impl fmt::Display for FramedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configuration => "invalid framed ISOAL configuration",
            Self::SduLength => "SDU exceeds framed ISOAL capacity",
            Self::TimeOffset => "framed ISOAL time offset must be 1..=0xffffff us",
            Self::TimestampOrder => "framed ISOAL SDU timestamps are out of order",
            Self::PayloadNumber => "framed ISOAL payload number is invalid or stale",
            Self::MalformedPdu => "malformed framed ISOAL PDU",
        })
    }
}

impl core::error::Error for FramedError {}

/// Validated limits shared by framed transmit and receive paths.
#[derive(Debug, Clone, Copy)]
pub struct Framed {
    max_pdu: u8,
    max_sdu: u16,
    sdu_interval_us: u32,
}

impl Framed {
    /// Validates `Max_PDU` (5..=251), `Max_SDU` (1..=4095), and `SDU_Interval`.
    /// Individual empty SDUs remain legal. SDU and ISO intervals need not match.
    ///
    /// # Errors
    /// Returns [`FramedError::Configuration`] for out-of-range values.
    pub fn new(max_pdu: u8, sdu_limit: u16, sdu_interval_us: u32) -> Result<Self, FramedError> {
        if !(5..=251).contains(&max_pdu)
            || !(1..=4095).contains(&sdu_limit)
            || !(0xff..=0x000f_ffff).contains(&sdu_interval_us)
        {
            return Err(FramedError::Configuration);
        }
        Ok(Self {
            max_pdu,
            max_sdu: sdu_limit,
            sdu_interval_us,
        })
    }

    /// Begins segmentation of ordered SDUs without copying their payloads.
    ///
    /// Timestamps use the controller's free-running microsecond clock, wrapping
    /// at 32 bits. Consecutive SDUs must be separated by at least half an SDU
    /// interval and less than half the clock range. Missing SDUs may be omitted;
    /// the receiver recovers their cadence from subsequent timestamps.
    ///
    /// # Errors
    /// Rejects oversized SDUs or non-increasing/ambiguous timestamp order.
    pub fn segment<'a>(
        &self,
        sdus: &'a [TimedSdu<'a>],
    ) -> Result<FramedFragments<'a>, FramedError> {
        for sdu in sdus {
            if sdu.payload.len() > usize::from(self.max_sdu) {
                return Err(FramedError::SduLength);
            }
        }
        for pair in sdus.windows(2) {
            let delta = pair[1].timestamp_us.wrapping_sub(pair[0].timestamp_us);
            if delta < self.sdu_interval_us.div_ceil(2) || delta >= (1 << 31) {
                return Err(FramedError::TimestampOrder);
            }
        }
        Ok(FramedFragments {
            profile: *self,
            remaining: sdus,
            offset: 0,
            continuation: false,
        })
    }
}

/// Source SDU and its reference time in the transmitting controller's clock.
#[derive(Debug, Clone, Copy)]
pub struct TimedSdu<'a> {
    /// Reference time before the CIG reference point or BIG anchor point.
    pub timestamp_us: u32,
    /// Complete SDU, possibly empty.
    pub payload: &'a [u8],
}

/// Borrowed SDU stream segmented into framed PDU payloads.
#[derive(Debug)]
pub struct FramedFragments<'a> {
    profile: Framed,
    remaining: &'a [TimedSdu<'a>],
    offset: usize,
    continuation: bool,
}

impl FramedFragments<'_> {
    /// Whether all supplied SDUs have been consumed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.remaining.is_empty()
    }

    /// Fills one original PDU (LLID 2) for this event's CIG/BIG reference point.
    ///
    /// Aggregates whole SDUs and fragments the last SDU when needed. Continuation
    /// PDUs may use later event anchors. Future SDUs wait; an empty return value
    /// is padding. At most one segment from an SDU appears in each PDU.
    ///
    /// # Errors
    /// Rejects an SDU older than the 24-bit offset range. State is unchanged on
    /// error, allowing the caller to discard/requeue expired source data.
    pub fn next_pdu(&mut self, reference_anchor_us: u32) -> Result<Vec<u8>, FramedError> {
        let mut remaining = self.remaining;
        let mut offset = self.offset;
        let mut continuation = self.continuation;
        let mut output = Vec::with_capacity(usize::from(self.profile.max_pdu));
        while let Some(sdu) = remaining.first() {
            let overhead = if continuation { 2 } else { 5 };
            let available = usize::from(self.profile.max_pdu) - output.len();
            if available < overhead {
                break;
            }
            let time_offset = reference_anchor_us.wrapping_sub(sdu.timestamp_us);
            if !continuation {
                if time_offset == 0 || time_offset >= (1 << 31) {
                    break;
                }
                if time_offset > 0x00ff_ffff {
                    return Err(FramedError::TimeOffset);
                }
            }
            let count = (sdu.payload.len() - offset).min(available - overhead);
            let complete = offset + count == sdu.payload.len();
            output.push(u8::from(continuation) | (u8::from(complete) << 1));
            // Bounded by Max_PDU <= 251, including the two-octet header.
            output.push(u8::try_from(count + overhead - 2).map_err(|_| FramedError::SduLength)?);
            if !continuation {
                output.extend_from_slice(&time_offset.to_le_bytes()[..3]);
            }
            output.extend_from_slice(&sdu.payload[offset..offset + count]);
            if !complete {
                offset += count;
                continuation = true;
                break;
            }
            remaining = &remaining[1..];
            offset = 0;
            continuation = false;
        }
        self.remaining = remaining;
        self.offset = offset;
        self.continuation = continuation;
        Ok(output)
    }
}

/// Received original PDU after Link Layer integrity verification.
#[derive(Debug, Clone, Copy)]
pub struct FramedPdu<'a> {
    /// Must be [`FRAMED_LLID`].
    pub llid: u8,
    /// Segmentation headers and data, excluding LL header and MIC.
    pub payload: &'a [u8],
}

/// Upper-layer SDU report. Losses never expose partial audio bytes.
#[derive(Debug, PartialEq, Eq)]
pub enum FramedReport {
    /// Correctly recombined SDU, including zero-length SDUs.
    Valid {
        /// Wrapping HCI `Packet_Sequence_Number`.
        sequence_number: u16,
        /// Reconstructed SDU synchronization reference in microseconds.
        timestamp_us: u32,
        /// Complete SDU bytes.
        payload: Vec<u8>,
    },
    /// Consecutive lost SDUs, compressed to bound memory during long outages.
    /// The host adapter must issue one lost-data report per interval, incrementing
    /// sequence number and timestamp by one and `SDU_Interval` respectively.
    Lost {
        /// First missing SDU's wrapping sequence number.
        first_sequence_number: u16,
        /// First missing SDU's synchronization reference.
        first_timestamp_us: u32,
        /// Number of missing SDUs; always positive.
        count: u32,
    },
}

#[derive(Debug)]
struct PendingSdu {
    payload: Vec<u8>,
    valid: bool,
}

/// Stateful framed receiver with bounded SDU buffers and loss accounting.
#[derive(Debug)]
pub struct FramedReceiver {
    profile: Framed,
    synchronization_delay_us: i32,
    next_sequence: u16,
    next_timestamp: Option<u32>,
    last_payload_number: Option<u64>,
    pending: Option<PendingSdu>,
}

impl FramedReceiver {
    /// Creates a receiver. Use sequence 0 when creating a CIS/BIS.
    ///
    /// `synchronization_delay_us` follows Core section 3.2.1:
    /// - CIS central to peripheral: `CIS_Sync_Delay` + `SDU_Interval` + FT * `ISO_Interval`.
    /// - CIS peripheral to central: `CIS_Sync_Delay` - `CIG_Sync_Delay`.
    /// - BIS: `BIG_Sync_Delay` + `SDU_Interval` + `ISO_Interval`.
    ///
    /// Supply an initial expected synchronization reference when known. With
    /// `None`, cadence starts at the first received start segment; losses before
    /// that point cannot be inferred. Adjacent observations/flush watermarks
    /// must be less than 2^31 microseconds apart to resolve clock wrap.
    #[must_use]
    pub fn new(
        profile: Framed,
        synchronization_delay_us: i32,
        first_sequence_number: u16,
        first_timestamp_us: Option<u32>,
    ) -> Self {
        Self {
            profile,
            synchronization_delay_us,
            next_sequence: first_sequence_number,
            next_timestamp: first_timestamp_us,
            last_payload_number: None,
            pending: None,
        }
    }

    /// Recombines a PDU; `None` means missing, CRC-failed, or MIC-failed data.
    ///
    /// Payload numbers are original 39-bit ISO payload counts. Gaps invalidate
    /// an unfinished SDU; retransmissions, duplicates and stale payloads must not
    /// be submitted. Starts following a gap restore framing. RFU bits are ignored.
    ///
    /// # Errors
    /// Rejects invalid payload order without changing state. Malformed PDUs or
    /// backward timestamps consume the payload number and invalidate any pending
    /// SDU, but keep its cadence. Its loss is reported on completion, a later start,
    /// or [`Self::expire_before`]. No completed report is lost on an error.
    pub fn receive(
        &mut self,
        payload_number: u64,
        reference_anchor_us: u32,
        pdu: Option<FramedPdu<'_>>,
    ) -> Result<Vec<FramedReport>, FramedError> {
        if payload_number > PAYLOAD_MASK {
            return Err(FramedError::PayloadNumber);
        }
        let delta = self
            .last_payload_number
            .map(|last| payload_number.wrapping_sub(last) & PAYLOAD_MASK);
        if delta.is_some_and(|distance| distance == 0 || distance >= (1 << 38)) {
            return Err(FramedError::PayloadNumber);
        }
        self.last_payload_number = Some(payload_number);
        if delta.is_some_and(|distance| distance != 1) || pdu.is_none() {
            self.invalidate_pending();
        }
        let Some(pdu) = pdu else {
            return Ok(Vec::new());
        };
        let segments = match self.validate_pdu(pdu, reference_anchor_us) {
            Ok(segments) => segments,
            Err(error) => {
                self.invalidate_pending();
                return Err(error);
            }
        };
        let mut reports = Vec::new();
        for segment in segments {
            if let Some(timestamp) = segment.timestamp {
                if self.pending.is_some() {
                    self.report_lost(1, &mut reports);
                }
                if let Some(expected) = self.next_timestamp {
                    let delta = timestamp.wrapping_sub(expected);
                    if delta < (1 << 31) {
                        let count = (delta + self.profile.sdu_interval_us / 2)
                            / self.profile.sdu_interval_us;
                        self.report_lost(count, &mut reports);
                    }
                }
                self.next_timestamp = Some(timestamp);
                self.pending = Some(PendingSdu {
                    payload: Vec::new(),
                    valid: true,
                });
            }
            if let Some(pending) = self.pending.as_mut() {
                if pending.payload.len() + segment.payload.len() > usize::from(self.profile.max_sdu)
                {
                    pending.valid = false;
                    pending.payload.clear();
                }
                if pending.valid {
                    pending.payload.extend_from_slice(segment.payload);
                }
                if segment.complete {
                    // Empty SDUs require one SC=0, CMPLT=1 segment (Core 2.2).
                    if segment.timestamp.is_none() && pending.payload.is_empty() {
                        pending.valid = false;
                    }
                    self.finish(&mut reports);
                }
            }
        }
        Ok(reports)
    }

    /// Reports all SDUs strictly before a caller-confirmed flush watermark lost.
    ///
    /// Call only after the Link Layer's delivery/flush deadline has passed for
    /// these synchronization references. This maintains cadence through complete
    /// outages, when no later segment can supply a timestamp. Padding itself
    /// never proves an SDU expired. An unknown initial reference yields no report.
    /// Old/equal watermarks are ignored; clocks use wrapping 32-bit arithmetic.
    #[must_use]
    pub fn expire_before(&mut self, timestamp_us: u32) -> Vec<FramedReport> {
        let mut reports = Vec::new();
        if let Some(expected) = self.next_timestamp {
            let delta = timestamp_us.wrapping_sub(expected);
            if delta > 0 && delta < (1 << 31) {
                self.report_lost(delta.div_ceil(self.profile.sdu_interval_us), &mut reports);
            }
        }
        reports
    }

    fn invalidate_pending(&mut self) {
        if let Some(pending) = self.pending.as_mut() {
            pending.valid = false;
            pending.payload.clear();
        }
    }

    fn report_lost(&mut self, count: u32, reports: &mut Vec<FramedReport>) {
        if count == 0 {
            return;
        }
        if let Some(timestamp) = self.next_timestamp {
            reports.push(FramedReport::Lost {
                first_sequence_number: self.next_sequence,
                first_timestamp_us: timestamp,
                count,
            });
            let bytes = count.to_le_bytes();
            self.next_sequence = self
                .next_sequence
                .wrapping_add(u16::from_le_bytes([bytes[0], bytes[1]]));
            self.next_timestamp =
                Some(timestamp.wrapping_add(count * self.profile.sdu_interval_us));
            self.pending = None;
        }
    }

    fn finish(&mut self, reports: &mut Vec<FramedReport>) {
        if self.pending.as_ref().is_some_and(|sdu| !sdu.valid) {
            self.report_lost(1, reports);
        } else if let (Some(pending), Some(timestamp)) = (self.pending.take(), self.next_timestamp)
        {
            reports.push(FramedReport::Valid {
                sequence_number: self.next_sequence,
                timestamp_us: timestamp,
                payload: pending.payload,
            });
            self.next_sequence = self.next_sequence.wrapping_add(1);
            self.next_timestamp = Some(timestamp.wrapping_add(self.profile.sdu_interval_us));
        }
    }

    fn validate_pdu<'a>(
        &self,
        pdu: FramedPdu<'a>,
        anchor: u32,
    ) -> Result<Vec<Segment<'a>>, FramedError> {
        if pdu.llid != FRAMED_LLID || pdu.payload.len() > usize::from(self.profile.max_pdu) {
            return Err(FramedError::MalformedPdu);
        }
        let mut bytes = pdu.payload;
        let mut segments = Vec::new();
        let mut expected = self.next_timestamp;
        let mut active = self.pending.is_some();
        while !bytes.is_empty() {
            let header = bytes.get(..2).ok_or(FramedError::MalformedPdu)?;
            let continuation = header[0] & 1 != 0;
            let complete = header[0] & 2 != 0;
            let size = usize::from(header[1]);
            let data = bytes.get(2..2 + size).ok_or(FramedError::MalformedPdu)?;
            bytes = &bytes[2 + size..];
            // Incomplete SDUs continue in another PDU, never another segment
            // of this PDU. A continuation can therefore only be first.
            if (!complete && !bytes.is_empty()) || (continuation && !segments.is_empty()) {
                return Err(FramedError::MalformedPdu);
            }
            let (timestamp, payload) = if continuation {
                (None, data)
            } else {
                let offset = data.get(..3).ok_or(FramedError::MalformedPdu)?;
                let offset = u32::from_le_bytes([offset[0], offset[1], offset[2], 0]);
                if offset == 0 {
                    return Err(FramedError::TimeOffset);
                }
                let timestamp = anchor
                    .wrapping_add_signed(self.synchronization_delay_us)
                    .wrapping_sub(offset);
                if active {
                    expected = expected.map(|time| time.wrapping_add(self.profile.sdu_interval_us));
                }
                if let Some(expected) = expected {
                    let delta = timestamp.wrapping_sub(expected);
                    if delta >= (1 << 31)
                        && expected.wrapping_sub(timestamp)
                            >= self.profile.sdu_interval_us.div_ceil(2)
                    {
                        return Err(FramedError::TimestampOrder);
                    }
                }
                expected = Some(timestamp);
                active = true;
                (Some(timestamp), &data[3..])
            };
            if complete && active {
                expected = expected.map(|time| time.wrapping_add(self.profile.sdu_interval_us));
                active = false;
            }
            segments.push(Segment {
                timestamp,
                payload,
                complete,
            });
        }
        Ok(segments)
    }
}

struct Segment<'a> {
    timestamp: Option<u32>,
    payload: &'a [u8],
    complete: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn pdu(payload: &[u8]) -> FramedPdu<'_> {
        FramedPdu { llid: 2, payload }
    }

    fn valid(sequence_number: u16, timestamp_us: u32, payload: &[u8]) -> FramedReport {
        FramedReport::Valid {
            sequence_number,
            timestamp_us,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn core_wire_vector_aggregates_and_fragments_with_original_offsets() {
        let profile = Framed::new(13, 100, 1000).unwrap();
        let sdus = [
            TimedSdu {
                timestamp_us: 1000,
                payload: &[10, 11],
            },
            TimedSdu {
                timestamp_us: 2000,
                payload: &[20, 21, 22, 23],
            },
        ];
        let mut tx = profile.segment(&sdus).unwrap();
        let first = tx.next_pdu(3000).unwrap();
        let second = tx.next_pdu(4000).unwrap();
        assert_eq!(first, [2, 5, 0xd0, 7, 0, 10, 11, 0, 4, 0xe8, 3, 0, 20]);
        assert_eq!(second, [3, 3, 21, 22, 23]);
        assert!(tx.is_complete());
        assert_eq!(tx.next_pdu(4000).unwrap(), []);
        let mut rx = FramedReceiver::new(profile, 15_000, 0, None);
        assert_eq!(
            rx.receive(0, 3000, Some(pdu(&first))).unwrap(),
            [valid(0, 16_000, &[10, 11])]
        );
        assert_eq!(
            rx.receive(1, 4000, Some(pdu(&second))).unwrap(),
            [valid(1, 17_000, &[20, 21, 22, 23])]
        );
    }

    #[test]
    fn zero_length_sdu_padding_future_data_and_minimum_pdu() {
        let profile = Framed::new(5, 4, 1000).unwrap();
        let sdus = [
            TimedSdu {
                timestamp_us: 1000,
                payload: &[],
            },
            TimedSdu {
                timestamp_us: 2000,
                payload: &[42],
            },
        ];
        let mut tx = profile.segment(&sdus).unwrap();
        assert_eq!(tx.next_pdu(1000).unwrap(), []);
        assert_eq!(tx.next_pdu(2000).unwrap(), [2, 3, 0xe8, 3, 0]);
        assert_eq!(tx.next_pdu(2000).unwrap(), []);
        let first = tx.next_pdu(3000).unwrap();
        assert_eq!(first, [0, 3, 0xe8, 3, 0]);
        let mut rx = FramedReceiver::new(profile, 0, 0, None);
        assert_eq!(rx.receive(0, 3000, Some(pdu(&[]))).unwrap(), []);
        assert_eq!(rx.receive(1, 3000, Some(pdu(&first))).unwrap(), []);
        assert_eq!(
            rx.receive(2, 4000, Some(pdu(&tx.next_pdu(4000).unwrap())))
                .unwrap(),
            [valid(0, 2000, &[42])]
        );
    }

    #[test]
    fn missing_fragments_and_whole_sdus_report_loss_without_partial_audio() {
        let profile = Framed::new(8, 100, 1000).unwrap();
        let mut rx = FramedReceiver::new(profile, 0, 0, Some(1000));
        assert_eq!(
            rx.receive(0, 3000, Some(pdu(&[0, 6, 0xd0, 7, 0, 1, 2, 3])))
                .unwrap(),
            []
        );
        assert_eq!(rx.receive(1, 3000, None).unwrap(), []);
        let reports = rx
            .receive(3, 4000, Some(pdu(&[2, 4, 0xe8, 3, 0, 9])))
            .unwrap();
        assert_eq!(
            reports,
            [
                FramedReport::Lost {
                    first_sequence_number: 0,
                    first_timestamp_us: 1000,
                    count: 1
                },
                FramedReport::Lost {
                    first_sequence_number: 1,
                    first_timestamp_us: 2000,
                    count: 1
                },
                valid(2, 3000, &[9]),
            ]
        );
        assert_eq!(
            rx.expire_before(7000),
            [FramedReport::Lost {
                first_sequence_number: 3,
                first_timestamp_us: 4000,
                count: 3
            }]
        );
        assert_eq!(rx.expire_before(7000), []);
    }

    #[test]
    fn clock_payload_and_sdu_sequence_wrap_independently() {
        let profile = Framed::new(20, 5, 1000).unwrap();
        let mut rx = FramedReceiver::new(profile, -100, u16::MAX, Some(u32::MAX - 499));
        let first = [2, 4, 0x90, 1, 0, 8];
        assert_eq!(
            rx.receive(PAYLOAD_MASK, 0, Some(pdu(&first))).unwrap(),
            [valid(u16::MAX, u32::MAX - 499, &[8])]
        );
        assert_eq!(
            rx.receive(0, 1000, Some(pdu(&first))).unwrap(),
            [valid(0, 500, &[8])]
        );
        assert_eq!(
            rx.receive(0, 1000, Some(pdu(&first))),
            Err(FramedError::PayloadNumber)
        );
        assert_eq!(
            rx.receive(PAYLOAD_MASK, 1000, None),
            Err(FramedError::PayloadNumber)
        );
    }

    #[test]
    fn malformed_packets_are_atomic_and_recover_on_next_start() {
        let profile = Framed::new(25, 3, 1000).unwrap();
        for bad in [
            vec![2],
            vec![2, 3, 1],
            vec![2, 2, 1, 0],
            vec![2, 3, 0, 0, 0],
            vec![0, 3, 1, 0, 0, 3, 0],
            vec![2, 3, 1, 0, 0, 3, 0],
            vec![2, 3, 1, 0, 0, 2],
        ] {
            let mut rx = FramedReceiver::new(profile, 0, 0, Some(1000));
            assert!(rx.receive(0, 2000, Some(pdu(&bad))).is_err());
            assert_eq!(
                rx.receive(1, 2000, Some(pdu(&[2, 4, 0xe8, 3, 0, 8])))
                    .unwrap(),
                [valid(0, 1000, &[8])]
            );
        }
        let mut rx = FramedReceiver::new(profile, 0, 0, None);
        assert_eq!(
            rx.receive(0, 2000, Some(pdu(&[0xfe, 4, 0xe8, 3, 0, 8])))
                .unwrap(),
            [valid(0, 1000, &[8])]
        );
    }

    #[test]
    fn oversize_sdu_and_expired_partial_become_lost_and_orphans_are_discarded() {
        let profile = Framed::new(10, 3, 1000).unwrap();
        let mut rx = FramedReceiver::new(profile, 0, 0, Some(1000));
        assert_eq!(
            rx.receive(0, 2000, Some(pdu(&[0, 5, 0xe8, 3, 0, 1, 2])))
                .unwrap(),
            []
        );
        assert_eq!(
            rx.receive(1, 2000, Some(pdu(&[3, 2, 3, 4]))).unwrap(),
            [FramedReport::Lost {
                first_sequence_number: 0,
                first_timestamp_us: 1000,
                count: 1
            }]
        );
        assert_eq!(
            rx.receive(2, 3000, Some(pdu(&[0, 4, 0xe8, 3, 0, 5])))
                .unwrap(),
            []
        );
        assert_eq!(
            rx.expire_before(2001),
            [FramedReport::Lost {
                first_sequence_number: 1,
                first_timestamp_us: 2000,
                count: 1
            }]
        );
        assert_eq!(rx.receive(3, 3000, Some(pdu(&[3, 1, 6]))).unwrap(), []);
        assert_eq!(
            rx.receive(4, 4000, Some(pdu(&[0, 3, 0xe8, 3, 0]))).unwrap(),
            []
        );
        assert_eq!(
            rx.receive(5, 4000, Some(pdu(&[3, 0]))).unwrap(),
            [FramedReport::Lost {
                first_sequence_number: 2,
                first_timestamp_us: 3000,
                count: 1,
            }]
        );
    }

    #[test]
    fn lost_range_is_bounded_and_timestamp_drift_rounds_to_nearest_interval() {
        let profile = Framed::new(20, 10, 1000).unwrap();
        let mut rx = FramedReceiver::new(profile, 0, 0, Some(1000));
        let reports = rx.receive(0, 5000, Some(pdu(&[2, 3, 0xc4, 9, 0]))).unwrap();
        assert_eq!(
            reports,
            [
                FramedReport::Lost {
                    first_sequence_number: 0,
                    first_timestamp_us: 1000,
                    count: 2
                },
                valid(2, 2500, &[])
            ]
        );
        let reports = rx.expire_before(2_000_000_000);
        assert_eq!(reports.len(), 1);
        assert!(matches!(
            reports[0],
            FramedReport::Lost {
                count: 1_999_997,
                ..
            }
        ));
    }

    #[test]
    fn all_sdu_sizes_roundtrip_across_events_and_pdu_boundaries() {
        let data = [0xa5; 4095];
        for max_pdu in [5, 6, 17, 251] {
            let profile = Framed::new(max_pdu, 4095, 10_000).unwrap();
            for length in [0, 1, 2, 3, 5, 12, 246, 247, 500, 4095] {
                let sdus = [TimedSdu {
                    timestamp_us: 1000,
                    payload: &data[..length],
                }];
                let mut tx = profile.segment(&sdus).unwrap();
                let mut rx = FramedReceiver::new(profile, 20_000, 0, None);
                let mut reports = Vec::new();
                let mut counter = 0;
                while !tx.is_complete() {
                    let anchor = 2000 + u32::try_from(counter).unwrap() * 10_000;
                    let bytes = tx.next_pdu(anchor).unwrap();
                    assert!(bytes.len() <= usize::from(max_pdu));
                    reports.extend(rx.receive(counter, anchor, Some(pdu(&bytes))).unwrap());
                    counter += 1;
                }
                assert_eq!(reports, [valid(0, 21_000, &data[..length])]);
            }
        }
    }
}
