//! Cleartext `BIGInfo` and a single-BIS LC3 broadcast announcement.
//!
//! Wire layout: Core 6.0 Vol 6 Part B section 4.4.6.11, Zephyr v4.2.0
//! `pdu_big_info` and `bap_broadcast_source.c::encode_base_subgroup`.
//! These helpers do not schedule advertising or transmit BIS packets.

use crate::{bap::LC3_CODEC_ID, bis::BisParameters, le_audio::Lc3Config};
use alloc::{vec, vec::Vec};
use core::fmt;

/// Supported uncoded BIS PHYs; wire values differ from HCI PHY values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BroadcastPhy {
    Le1M = 0,
    Le2M = 1,
}

/// Unencrypted, segmentable-framed or unframed BIG parameters.
///
/// Offset describes the anchor relative to the start of the packet carrying
/// this information. Caller must refresh offset and payload count per event,
/// provide an entropy-generated seed, and enforce device-wide address rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BigInfo {
    pub offset: u16,
    /// False: 30 us units; true: 300 us units.
    pub offset_units_300_us: bool,
    /// Units of 1.25 ms, range 4..=3200.
    pub iso_interval: u16,
    pub num_bis: u8,
    pub nse: u8,
    pub bn: u8,
    pub sub_interval_us: u32,
    pub pto: u8,
    pub bis_spacing_us: u32,
    pub irc: u8,
    pub max_pdu: u8,
    pub seed_access_address: u32,
    pub sdu_interval_us: u32,
    pub max_sdu: u16,
    pub base_crc_init: u16,
    /// 37 data-channel bits, remaining bits zero.
    pub channel_map: [u8; 5],
    pub phy: BroadcastPhy,
    /// 39-bit counter for the described event's first subevent.
    pub payload_count: u64,
    pub framed: bool,
}

impl BigInfo {
    /// Validates field ranges, identities, channel map, and data-event timing.
    ///
    /// Does not reserve time for an optional BIG Control subevent or determine
    /// ISOAL feasibility; those require the scheduler and selected ISOAL mode.
    ///
    /// # Errors
    /// Returns invalid field, radio identity, or overlapping subevents.
    pub fn validate(&self) -> Result<(), BroadcastError> {
        let offset_us = u32::from(self.offset) * if self.offset_units_300_us { 300 } else { 30 };
        if self.offset > 0x3fff
            || offset_us <= 600
            || (self.offset_units_300_us && offset_us < 491_460)
            || !(4..=3200).contains(&self.iso_interval)
            || !(1..=31).contains(&self.num_bis)
            || !(1..=31).contains(&self.nse)
            || !(1..=7).contains(&self.bn)
            || !self.nse.is_multiple_of(self.bn)
            || !(1..=15).contains(&self.irc)
            || self.irc > self.nse / self.bn
            || self.pto > 15
            || (self.irc < self.nse / self.bn && self.pto == 0)
            || !(1..=251).contains(&self.max_pdu)
            || !(1..=4095).contains(&self.max_sdu)
            || !(255..=0x000f_ffff).contains(&self.sdu_interval_us)
            || self.sub_interval_us > 0x000f_ffff
            || self.bis_spacing_us > 0x000f_ffff
            || self.payload_count >= 1 << 39
            || self.channel_map[4] & 0xe0 != 0
            || self.channel_map.iter().map(|b| b.count_ones()).sum::<u32>() < 2
        {
            return Err(BroadcastError::Field);
        }
        for bis in 0..=self.num_bis {
            BisParameters::derive(self.seed_access_address, self.base_crc_init, bis)
                .map_err(|_| BroadcastError::Identity)?;
        }
        let mpt = match self.phy {
            BroadcastPhy::Le1M => (u32::from(self.max_pdu) + 10) * 8,
            BroadcastPhy::Le2M => (u32::from(self.max_pdu) + 11) * 4,
        };
        let end = u32::from(self.num_bis - 1) * self.bis_spacing_us
            + u32::from(self.nse - 1) * self.sub_interval_us
            + mpt;
        if self.sub_interval_us < mpt + 150
            || self.bis_spacing_us < mpt + 150
            || (self.bis_spacing_us < u32::from(self.nse) * self.sub_interval_us
                && self.sub_interval_us < u32::from(self.num_bis) * self.bis_spacing_us)
            || end + 150 > u32::from(self.iso_interval) * 1250
        {
            return Err(BroadcastError::Timing);
        }
        Ok(())
    }

    /// Encodes 33 data octets, without the ACAD AD length and type.
    ///
    /// # Errors
    /// Returns any error from [`Self::validate`].
    pub fn encode(&self) -> Result<[u8; 33], BroadcastError> {
        self.validate()?;
        let mut bytes = [0; 33];
        let first = u32::from(self.offset)
            | (u32::from(self.offset_units_300_us) << 14)
            | (u32::from(self.iso_interval) << 15)
            | (u32::from(self.num_bis) << 27);
        bytes[..4].copy_from_slice(&first.to_le_bytes());
        let second = u32::from(self.nse)
            | (u32::from(self.bn) << 5)
            | (self.sub_interval_us << 8)
            | (u32::from(self.pto) << 28);
        bytes[4..8].copy_from_slice(&second.to_le_bytes());
        let spacing = self.bis_spacing_us | (u32::from(self.irc) << 20);
        bytes[8..11].copy_from_slice(&spacing.to_le_bytes()[..3]);
        bytes[11] = self.max_pdu;
        bytes[13..17].copy_from_slice(&self.seed_access_address.to_le_bytes());
        let sdu = self.sdu_interval_us | (u32::from(self.max_sdu) << 20);
        bytes[17..21].copy_from_slice(&sdu.to_le_bytes());
        bytes[21..23].copy_from_slice(&self.base_crc_init.to_le_bytes());
        bytes[23..28].copy_from_slice(&self.channel_map);
        bytes[27] |= (self.phy as u8) << 5;
        let count = self.payload_count | (u64::from(self.framed) << 39);
        bytes[28..33].copy_from_slice(&count.to_le_bytes()[..5]);
        Ok(bytes)
    }

    /// Parses exactly one cleartext `BIGInfo` value.
    ///
    /// # Errors
    /// Rejects encrypted values, Coded PHY, nonzero framing-mode/RFU byte,
    /// malformed lengths, and invalid values checked by [`Self::validate`].
    pub fn parse(bytes: &[u8]) -> Result<Self, BroadcastError> {
        // ponytail: 33-byte uncoded segmentable subset; add encryption/Coded with radio support.
        if bytes.len() == 57 {
            return Err(BroadcastError::Unsupported);
        }
        let bytes: &[u8; 33] = bytes.try_into().map_err(|_| BroadcastError::Length)?;
        if bytes[12] != 0 {
            return Err(BroadcastError::Unsupported);
        }
        let phy = match bytes[27] >> 5 {
            0 => BroadcastPhy::Le1M,
            1 => BroadcastPhy::Le2M,
            _ => return Err(BroadcastError::Unsupported),
        };
        let first = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let second = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let spacing = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], 0]);
        let sdu = u32::from_le_bytes([bytes[17], bytes[18], bytes[19], bytes[20]]);
        let channel_map = [bytes[23], bytes[24], bytes[25], bytes[26], bytes[27] & 0x1f];
        let mut count = [0; 8];
        count[..5].copy_from_slice(&bytes[28..33]);
        count[4] &= 0x7f;
        let info = Self {
            offset: u16::from_le_bytes([bytes[0], bytes[1]]) & 0x3fff,
            offset_units_300_us: first & (1 << 14) != 0,
            iso_interval: u16::try_from((first >> 15) & 0x0fff)
                .map_err(|_| BroadcastError::Field)?,
            num_bis: bytes[3] >> 3,
            nse: bytes[4] & 0x1f,
            bn: bytes[4] >> 5,
            sub_interval_us: (second >> 8) & 0x000f_ffff,
            pto: bytes[7] >> 4,
            bis_spacing_us: spacing & 0x000f_ffff,
            irc: bytes[10] >> 4,
            max_pdu: bytes[11],
            seed_access_address: u32::from_le_bytes([bytes[13], bytes[14], bytes[15], bytes[16]]),
            sdu_interval_us: sdu & 0x000f_ffff,
            max_sdu: u16::try_from(sdu >> 20).map_err(|_| BroadcastError::Field)?,
            base_crc_init: u16::from_le_bytes([bytes[21], bytes[22]]),
            channel_map,
            phy,
            payload_count: u64::from_le_bytes(count),
            framed: bytes[32] & 0x80 != 0,
        };
        info.validate()?;
        Ok(info)
    }

    /// Encodes the ACAD AD structure; this is not periodic advertising data.
    ///
    /// # Errors
    /// Returns any error from [`Self::validate`].
    pub fn acad(&self) -> Result<[u8; 35], BroadcastError> {
        let mut bytes = [0; 35];
        bytes[..2].copy_from_slice(&[34, 0x2c]);
        bytes[2..].copy_from_slice(&self.encode()?);
        Ok(bytes)
    }
}

/// Minimal LC3 BASE: one subgroup, BIS index 1, no BIS-specific overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SingleBisBase {
    /// 24-bit presentation delay in microseconds.
    pub presentation_delay_us: u32,
    pub config: Lc3Config,
    /// Streaming Audio Contexts assigned-number bitfield, bits 0..11.
    pub streaming_contexts: u16,
}

impl SingleBisBase {
    /// Encodes a BASE without its service UUID or AD envelope.
    ///
    /// # Errors
    /// Rejects out-of-range delay/contexts or an invalid LC3 configuration.
    pub fn encode(&self) -> Result<Vec<u8>, BroadcastError> {
        if self.presentation_delay_us > 0x00ff_ffff || self.streaming_contexts & 0xf000 != 0 {
            return Err(BroadcastError::Field);
        }
        self.config
            .sdu_length()
            .map_err(|_| BroadcastError::Codec)?;
        let config = self.config.encode().map_err(|_| BroadcastError::Codec)?;
        let mut bytes = self.presentation_delay_us.to_le_bytes()[..3].to_vec();
        bytes.extend_from_slice(&[1, 1]);
        bytes.extend_from_slice(&LC3_CODEC_ID);
        bytes.push(u8::try_from(config.len()).map_err(|_| BroadcastError::Length)?);
        bytes.extend_from_slice(&config);
        bytes.extend_from_slice(&[4, 3, 2]);
        bytes.extend_from_slice(&self.streaming_contexts.to_le_bytes());
        bytes.extend_from_slice(&[1, 0]);
        Ok(bytes)
    }

    /// Encodes Basic Audio Announcement service data for periodic advertising.
    ///
    /// # Errors
    /// Returns invalid BASE fields or a value exceeding the AD length limit.
    pub fn periodic_advertising_data(&self) -> Result<Vec<u8>, BroadcastError> {
        let base = self.encode()?;
        let length = u8::try_from(base.len() + 3).map_err(|_| BroadcastError::Length)?;
        let mut bytes = vec![length, 0x16, 0x51, 0x18];
        bytes.extend_from_slice(&base);
        Ok(bytes)
    }
}

/// Broadcast Audio Announcement service data for extended advertising.
///
/// Caller generates a random 24-bit Broadcast ID; this helper provides no entropy.
///
/// # Errors
/// Rejects IDs exceeding 24 bits.
pub fn broadcast_audio_announcement(id: u32) -> Result<[u8; 7], BroadcastError> {
    if id > 0x00ff_ffff {
        return Err(BroadcastError::Field);
    }
    let mut bytes = [6, 0x16, 0x52, 0x18, 0, 0, 0];
    bytes[4..].copy_from_slice(&id.to_le_bytes()[..3]);
    Ok(bytes)
}

/// Invalid or unsupported broadcast parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastError {
    Length,
    Field,
    Identity,
    Timing,
    Unsupported,
    Codec,
}

impl fmt::Display for BroadcastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "broadcast error: {self:?}")
    }
}

impl core::error::Error for BroadcastError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> BigInfo {
        BigInfo {
            offset: 1234,
            offset_units_300_us: false,
            iso_interval: 24,
            num_bis: 2,
            nse: 6,
            bn: 2,
            sub_interval_us: 2500,
            pto: 3,
            bis_spacing_us: 1000,
            irc: 2,
            max_pdu: 80,
            seed_access_address: 0x8c88_bed6,
            sdu_interval_us: 10000,
            max_sdu: 80,
            base_crc_init: 0x1234,
            channel_map: [0xff, 0xff, 0xff, 0xff, 0x1f],
            phy: BroadcastPhy::Le2M,
            payload_count: 0x12_1234_5678,
            framed: true,
        }
    }

    #[test]
    fn biginfo_matches_zephyr_c_packing_vector() {
        // Zephyr v4.2.0 pdu.h PDU_BIG_INFO_*_SET + util.c util_set_bits,
        // compiled on a little-endian host; two interleaved BIS, three groups.
        let bytes = [
            0xd2, 0x04, 0x0c, 0x10, 0x46, 0xc4, 0x09, 0x30, 0xe8, 0x03, 0x20, 0x50, 0x00, 0xd6,
            0xbe, 0x88, 0x8c, 0x10, 0x27, 0x00, 0x05, 0x34, 0x12, 0xff, 0xff, 0xff, 0xff, 0x3f,
            0x78, 0x56, 0x34, 0x12, 0x92,
        ];
        assert_eq!(fixture().encode().unwrap(), bytes);
        assert_eq!(BigInfo::parse(&bytes).unwrap(), fixture());
        let acad = fixture().acad().unwrap();
        assert_eq!(&acad[..2], &[34, 0x2c]);
        assert_eq!(&acad[2..], &bytes);
        for length in 0..33 {
            assert_eq!(
                BigInfo::parse(&bytes[..length]),
                Err(BroadcastError::Length)
            );
        }
        assert_eq!(BigInfo::parse(&[0; 57]), Err(BroadcastError::Unsupported));
        for (offset, value) in [(12, 1), (27, 0x5f), (27, 0xff)] {
            let mut bad = bytes;
            bad[offset] = value;
            assert_eq!(BigInfo::parse(&bad), Err(BroadcastError::Unsupported));
        }
    }

    #[test]
    fn range_and_identity_failures_are_rejected_before_packing() {
        let changes: [fn(&mut BigInfo); 17] = [
            |b| b.bn = 0,
            |b| b.bn = 4,
            |b| b.num_bis = 0,
            |b| b.num_bis = 32,
            |b| b.nse = 32,
            |b| b.irc = 4,
            |b| b.pto = 0,
            |b| b.pto = 16,
            |b| b.max_pdu = 252,
            |b| b.max_sdu = 4096,
            |b| b.sdu_interval_us = 254,
            |b| b.sub_interval_us = 1 << 20,
            |b| b.bis_spacing_us = 1 << 20,
            |b| b.payload_count = 1 << 39,
            |b| b.channel_map = [1, 0, 0, 0, 0],
            |b| b.channel_map[4] = 0x20,
            |b| b.iso_interval = 3201,
        ];
        for change in changes {
            let mut info = fixture();
            change(&mut info);
            assert_eq!(info.encode(), Err(BroadcastError::Field));
        }
        let mut info = fixture();
        info.seed_access_address = 0x8e89_bed6;
        assert_eq!(info.encode(), Err(BroadcastError::Identity));
    }

    #[test]
    fn offset_quantization_and_event_spacing_boundaries() {
        let mut info = fixture();
        info.offset = 20;
        assert_eq!(info.encode(), Err(BroadcastError::Field));
        info.offset = 21;
        assert!(info.encode().is_ok());
        info.offset = 1638;
        info.offset_units_300_us = true;
        assert_eq!(info.encode(), Err(BroadcastError::Field));
        info.offset = 1639;
        assert_eq!(BigInfo::parse(&info.encode().unwrap()).unwrap(), info);
        info.sub_interval_us = 1999;
        assert_eq!(info.encode(), Err(BroadcastError::Timing));
        info.sub_interval_us = 2000;
        assert!(info.encode().is_ok());
        info.iso_interval = 4;
        assert_eq!(info.encode(), Err(BroadcastError::Timing));
        info.num_bis = 1;
        info.nse = 1;
        info.bn = 1;
        info.irc = 1;
        info.pto = 0;
        info.phy = BroadcastPhy::Le1M;
        info.sub_interval_us = 870;
        info.bis_spacing_us = 870;
        assert!(info.encode().is_ok());
        info.sub_interval_us = 869;
        assert_eq!(info.encode(), Err(BroadcastError::Timing));
    }

    #[test]
    fn single_bis_base_and_announcements_have_expected_wire_fields() {
        let mut base = SingleBisBase {
            presentation_delay_us: 40000,
            config: Lc3Config::parse(&[2, 1, 3, 2, 2, 1, 3, 4, 40, 0]).unwrap(),
            streaming_contexts: 4,
        };
        let expected = [
            0x40, 0x9c, 0, 1, 1, 6, 0, 0, 0, 0, 10, 2, 1, 3, 2, 2, 1, 3, 4, 40, 0, 4, 3, 2, 4, 0,
            1, 0,
        ];
        assert_eq!(base.encode().unwrap(), expected);
        let ad = base.periodic_advertising_data().unwrap();
        assert_eq!(&ad[..4], &[31, 0x16, 0x51, 0x18]);
        assert_eq!(&ad[4..], expected);
        assert_eq!(
            broadcast_audio_announcement(0x12_3456).unwrap(),
            [6, 0x16, 0x52, 0x18, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            broadcast_audio_announcement(1 << 24),
            Err(BroadcastError::Field)
        );
        base.presentation_delay_us = 1 << 24;
        assert_eq!(base.encode(), Err(BroadcastError::Field));
        base.presentation_delay_us = 0;
        base.streaming_contexts = 0x1000;
        assert_eq!(base.encode(), Err(BroadcastError::Field));
    }
}
