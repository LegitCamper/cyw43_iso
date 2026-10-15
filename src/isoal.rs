//! Unframed ISOAL segmentation/recombination for one SDU per ISO interval.
//!
//! Bluetooth Core 6.0, Vol 6, Part G, sections 2.1 and 4 specify LLID values,
//! fixed fragment counts, padding, and permitted invalid-SDU discard behavior:
//! <https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-60/out/en/low-energy-controller/isochronous-adaptation-layer.html>
//!
//! The Link Layer must supply original payload slots in order after removing
//! retransmissions, MIC, and LL headers; missing/CRC-failed slots become `None`.
//! Call once for every SDU interval, including intervals containing only losses.
//! Constant alignment of SDU generation to transport timing is required upstream.
//! This code does not establish CIS/BIS, schedule radio events, assign stream
//! sequence numbers, or reconstruct synchronization references.

use alloc::vec::Vec;
use core::{fmt, iter::FusedIterator};

/// Invalid configuration or caller-supplied SDU/burst shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoalError {
    /// Invalid maximum PDU size, burst number, SDU capacity, or ISO interval.
    Configuration,
    /// This profile requires identical ISO and SDU intervals.
    UnsupportedInterval,
    /// Input SDU exceeds the negotiated maximum.
    SduLength,
    /// Input slots do not cover exactly one burst of original payloads.
    PduCount,
}

impl fmt::Display for IsoalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configuration => "invalid unframed ISOAL configuration",
            Self::UnsupportedInterval => "unframed ISOAL requires one SDU per ISO interval",
            Self::SduLength => "SDU exceeds unframed ISOAL capacity",
            Self::PduCount => "unframed ISOAL burst has incorrect slot count",
        })
    }
}

impl core::error::Error for IsoalError {}

/// Unencrypted ISO Data PDU fields at the ISOAL/Link Layer boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnframedPdu<'a> {
    /// 0 is complete/end; 1 is start/continuation or empty padding.
    pub llid: u8,
    /// SDU fragment, excluding the Link Layer header and MIC.
    pub payload: &'a [u8],
}

/// One interval's SDU; malformed or missing data is conservatively discarded.
#[derive(Debug, PartialEq, Eq)]
pub enum UnframedSdu {
    /// Complete SDU, including a genuine zero-length SDU.
    Valid(Vec<u8>),
    /// Report to the upper layer as lost data, with no payload.
    Lost,
}

/// Validated unframed ISOAL profile with one SDU per interval.
#[derive(Debug, Clone, Copy)]
pub struct Unframed {
    max_pdu: u8,
    burst_number: u8,
    max_sdu: u16,
}

impl Unframed {
    /// Validates negotiated limits; PDU size excludes the LL header and MIC.
    ///
    /// Accepts active CIS burst numbers 1..=15; callers setting up a BIS must
    /// additionally enforce the BIS-specific limit of 7. ISO intervals are
    /// 5 ms..=4 s in 1.25 ms steps, and SDU intervals cannot exceed 0x0fffff us.
    /// This validates an already selected unframed
    /// configuration; a peer/host request for framed operation must be honored.
    ///
    /// # Errors
    /// Rejects PDU sizes outside 1..=251, burst numbers outside 1..=15, SDU
    /// capacities outside `1..=max_pdu * burst_number`, invalid ISO intervals,
    /// or unequal ISO and SDU intervals. Positive capacity selects an active
    /// profile; individual zero-length SDUs remain supported.
    pub fn new(
        max_pdu: u8,
        burst_number: u8,
        sdu_limit: u16,
        iso_interval_us: u32,
        sdu_interval_us: u32,
    ) -> Result<Self, IsoalError> {
        if !(1..=251).contains(&max_pdu)
            || !(1..=15).contains(&burst_number)
            || sdu_limit == 0
            || sdu_limit > u16::from(max_pdu) * u16::from(burst_number)
            || !(5_000..=4_000_000).contains(&iso_interval_us)
            || !iso_interval_us.is_multiple_of(1_250)
            || !(0xff..=0x000f_ffff).contains(&sdu_interval_us)
        {
            return Err(IsoalError::Configuration);
        }
        // ponytail: one SDU per interval; add USPI/UPPS grouping for multiple SDUs.
        if iso_interval_us != sdu_interval_us {
            return Err(IsoalError::UnsupportedInterval);
        }
        Ok(Self {
            max_pdu,
            burst_number,
            max_sdu: sdu_limit,
        })
    }

    /// Produces exactly BN original payloads, including any trailing padding.
    ///
    /// # Errors
    /// Returns [`IsoalError::SduLength`] if the SDU exceeds the negotiated limit.
    pub fn segment<'a>(&self, payload: &'a [u8]) -> Result<UnframedFragments<'a>, IsoalError> {
        if payload.len() > usize::from(self.max_sdu) {
            return Err(IsoalError::SduLength);
        }
        Ok(UnframedFragments {
            remaining: payload,
            max_pdu: self.max_pdu,
            slots_left: self.burst_number,
            ended: false,
        })
    }

    /// Recombines exactly one burst's original payload slots.
    ///
    /// `None` means lost or integrity-failed PDU; never pass its untrusted LLID
    /// or length as `Some`. Missing slots after a valid end are padding and do
    /// not invalidate the SDU. A valid empty SDU requires one LLID 0 packet;
    /// padding alone is lost data. Empty LLID 1 fragments before the end are
    /// tolerated, as are non-final fragments shorter than the maximum PDU.
    /// Invalid LLIDs, overflow, duplicate ends, missing ends, and data after an
    /// end cause a lost-SDU report.
    ///
    /// # Errors
    /// Returns [`IsoalError::PduCount`] unless exactly BN slots are supplied.
    /// Peer data errors return [`UnframedSdu::Lost`], preserving SDU cadence.
    pub fn recombine(&self, slots: &[Option<UnframedPdu<'_>>]) -> Result<UnframedSdu, IsoalError> {
        if slots.len() != usize::from(self.burst_number) {
            return Err(IsoalError::PduCount);
        }
        let mut payload = Vec::with_capacity(usize::from(self.max_sdu));
        let mut ended = false;
        for slot in slots {
            let Some(pdu) = slot else {
                if ended {
                    continue;
                }
                return Ok(UnframedSdu::Lost);
            };
            if pdu.llid > 1 || pdu.payload.len() > usize::from(self.max_pdu) {
                return Ok(UnframedSdu::Lost);
            }
            if ended {
                if pdu.llid != 1 || !pdu.payload.is_empty() {
                    return Ok(UnframedSdu::Lost);
                }
                continue;
            }
            if payload.len() + pdu.payload.len() > usize::from(self.max_sdu) {
                return Ok(UnframedSdu::Lost);
            }
            payload.extend_from_slice(pdu.payload);
            ended = pdu.llid == 0;
        }
        Ok(if ended {
            UnframedSdu::Valid(payload)
        } else {
            UnframedSdu::Lost
        })
    }
}

/// Allocation-free, exact-size iterator over one SDU's fragments and padding.
#[derive(Debug)]
pub struct UnframedFragments<'a> {
    remaining: &'a [u8],
    max_pdu: u8,
    slots_left: u8,
    ended: bool,
}

impl<'a> Iterator for UnframedFragments<'a> {
    type Item = UnframedPdu<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.slots_left == 0 {
            return None;
        }
        self.slots_left -= 1;
        if self.ended {
            return Some(UnframedPdu {
                llid: 1,
                payload: &[],
            });
        }
        let count = self.remaining.len().min(usize::from(self.max_pdu));
        let (payload, rest) = self.remaining.split_at(count);
        self.remaining = rest;
        self.ended = rest.is_empty();
        Some(UnframedPdu {
            llid: u8::from(!self.ended),
            payload,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let length = usize::from(self.slots_left);
        (length, Some(length))
    }
}

impl ExactSizeIterator for UnframedFragments<'_> {}
impl FusedIterator for UnframedFragments<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[expect(
        clippy::unnecessary_wraps,
        reason = "Test input slots use None for loss and Some for received PDUs"
    )]
    fn pdu(llid: u8, payload: &[u8]) -> Option<UnframedPdu<'_>> {
        Some(UnframedPdu { llid, payload })
    }

    #[test]
    fn core_section_2_1_three_fragments_plus_padding_vector() {
        let isoal = Unframed::new(3, 4, 12, 10_000, 10_000).unwrap();
        let payload = [1, 2, 3, 4, 5, 6, 7];
        let slots: Vec<_> = isoal.segment(&payload).unwrap().map(Some).collect();
        assert_eq!(
            slots,
            [
                pdu(1, &[1, 2, 3]),
                pdu(1, &[4, 5, 6]),
                pdu(0, &[7]),
                pdu(1, &[])
            ]
        );
        assert_eq!(
            isoal.recombine(&slots),
            Ok(UnframedSdu::Valid(payload.to_vec()))
        );
    }

    #[test]
    fn zero_length_sdu_differs_from_padding_only_interval() {
        let isoal = Unframed::new(4, 2, 8, 7_500, 7_500).unwrap();
        let slots: Vec<_> = isoal.segment(&[]).unwrap().map(Some).collect();
        assert_eq!(slots, [pdu(0, &[]), pdu(1, &[])]);
        assert_eq!(isoal.recombine(&slots), Ok(UnframedSdu::Valid(vec![])));
        assert_eq!(isoal.recombine(&[pdu(1, &[]); 2]), Ok(UnframedSdu::Lost));
        assert_eq!(isoal.recombine(&[None; 2]), Ok(UnframedSdu::Lost));
        assert_eq!(
            isoal.recombine(&[pdu(1, &[]), pdu(0, &[1])]),
            Ok(UnframedSdu::Valid(vec![1]))
        );
        assert_eq!(
            isoal.recombine(&[pdu(1, &[1]), pdu(0, &[2, 3, 4])]),
            Ok(UnframedSdu::Valid(vec![1, 2, 3, 4]))
        );
    }

    #[test]
    fn lost_data_and_missing_end_do_not_leak_partial_audio() {
        let isoal = Unframed::new(3, 3, 9, 10_000, 10_000).unwrap();
        for slots in [
            [pdu(1, &[1]), None, pdu(0, &[3])],
            [None, pdu(1, &[2]), pdu(0, &[3])],
            [pdu(1, &[1]), pdu(1, &[2]), None],
            [pdu(1, &[1]), pdu(1, &[2]), pdu(1, &[3])],
            [pdu(0, &[1]), pdu(0, &[]), pdu(1, &[])],
            [pdu(0, &[1]), pdu(1, &[2]), pdu(1, &[])],
            [pdu(2, &[1]), pdu(0, &[2]), pdu(1, &[])],
        ] {
            assert_eq!(isoal.recombine(&slots), Ok(UnframedSdu::Lost));
        }
        assert_eq!(
            isoal.recombine(&[pdu(0, &[1]), None, pdu(1, &[])]),
            Ok(UnframedSdu::Valid(vec![1]))
        );
    }

    #[test]
    fn negotiated_limits_hold_at_both_trust_boundaries() {
        assert!(Unframed::new(0, 1, 0, 10_000, 10_000).is_err());
        assert!(Unframed::new(1, 1, 0, 10_000, 10_000).is_err());
        assert!(Unframed::new(252, 1, 1, 10_000, 10_000).is_err());
        assert!(Unframed::new(1, 16, 1, 10_000, 10_000).is_err());
        assert!(Unframed::new(1, 2, 3, 10_000, 10_000).is_err());
        assert!(Unframed::new(1, 1, 1, 10_001, 10_001).is_err());
        assert!(Unframed::new(1, 1, 1, 1_050_000, 1_050_000).is_err());
        assert!(matches!(
            Unframed::new(1, 1, 1, 10_000, 5_000),
            Err(IsoalError::UnsupportedInterval)
        ));
        let isoal = Unframed::new(3, 2, 4, 10_000, 10_000).unwrap();
        assert!(matches!(isoal.segment(&[0; 5]), Err(IsoalError::SduLength)));
        assert_eq!(
            isoal.recombine(&[pdu(0, &[0; 4]), pdu(1, &[])]),
            Ok(UnframedSdu::Lost)
        );
        assert_eq!(
            isoal.recombine(&[pdu(1, &[0; 3]), pdu(0, &[0; 2])]),
            Ok(UnframedSdu::Lost)
        );
        assert_eq!(isoal.recombine(&[pdu(0, &[])]), Err(IsoalError::PduCount));
    }

    #[test]
    fn maximum_burst_roundtrips_all_sdu_sizes() {
        let isoal = Unframed::new(251, 15, 3765, 10_000, 10_000).unwrap();
        let payload = [0xa5; 3765];
        for length in 0..=payload.len() {
            let mut fragments = isoal.segment(&payload[..length]).unwrap();
            assert_eq!(fragments.len(), 15);
            let slots: Vec<_> = fragments.by_ref().map(Some).collect();
            assert!(fragments.next().is_none());
            assert_eq!(
                isoal.recombine(&slots),
                Ok(UnframedSdu::Valid(payload[..length].to_vec()))
            );
        }
    }
}
