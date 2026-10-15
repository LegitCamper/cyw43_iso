//! Software-only BAP LC3 codec configuration and unicast `QoS` bounds.
//!
//! Sources: Bluetooth Generic Audio Assigned Numbers and BAP, cross-checked
//! against Zephyr v4.2.0 `audio.h`, `lc3.h`, `codec.c`, `bap_stream.c`, and `iso.h`:
//! <https://github.com/zephyrproject-rtos/zephyr/tree/v4.2.0/subsys/bluetooth/audio>
//! <https://github.com/zephyrproject-rtos/zephyr/tree/v4.2.0/include/zephyr/bluetooth>
//!
//! This is configuration handling, not an LC3 encoder or an ASCS/PACS service.
//! Callers supply an already-identified LC3 `Codec_Specific_Configuration` value,
//! without its outer length or `Codec_ID`. Unknown LTV types are rejected rather
//! than silently discarded. Encoding produces canonical order and omits defaults.

use crate::iso::MAX_SDU_LENGTH;
use alloc::{vec, vec::Vec};
use core::fmt;

/// LC3 sample rates used by BAP 1.0 configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SamplingFrequency {
    /// 8 kHz.
    Hz8000 = 0x01,
    /// 16 kHz.
    Hz16000 = 0x03,
    /// 24 kHz.
    Hz24000 = 0x05,
    /// 32 kHz.
    Hz32000 = 0x06,
    /// 44.1 kHz; actual frame duration differs from nominal duration.
    Hz44100 = 0x07,
    /// 48 kHz.
    Hz48000 = 0x08,
}

impl SamplingFrequency {
    fn parse(value: u8) -> Result<Self, AudioError> {
        match value {
            1 => Ok(Self::Hz8000),
            3 => Ok(Self::Hz16000),
            5 => Ok(Self::Hz24000),
            6 => Ok(Self::Hz32000),
            7 => Ok(Self::Hz44100),
            8 => Ok(Self::Hz48000),
            _ => Err(AudioError::UnsupportedFrequency(value)),
        }
    }
}

/// Nominal LC3 frame duration, not necessarily the SDU interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameDuration {
    /// Nominal 7.5 ms.
    Ms7_5 = 0,
    /// Nominal 10 ms.
    Ms10 = 1,
}

/// LC3 configuration fields. Public field edits are checked before use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lc3Config {
    /// Sampling frequency assigned number.
    pub frequency: SamplingFrequency,
    /// Nominal frame duration assigned number.
    pub duration: FrameDuration,
    /// Audio Location bits 0..27; zero means a single unspecified channel.
    pub channel_allocation: u32,
    /// Encoded octets in each channel's codec frame.
    pub octets_per_frame: u16,
    /// Number of blocks per SDU, with one frame per channel in each block.
    pub frame_blocks_per_sdu: u8,
}

impl Lc3Config {
    /// Parses a complete LC3 `Codec_Specific_Configuration` LTV sequence.
    ///
    /// Missing channel allocation defaults to mono; missing block count to one.
    /// Duplicate and unknown types are rejected by this strict parser.
    ///
    /// # Errors
    /// Returns an error for malformed LTVs, missing required fields, unsupported
    /// values, zero frame sizes/counts, or reserved Audio Location bits.
    pub fn parse(mut bytes: &[u8]) -> Result<Self, AudioError> {
        let mut fields: [Option<&[u8]>; 5] = [None; 5];
        while let Some((&length, rest)) = bytes.split_first() {
            if length == 0 {
                return Err(AudioError::MalformedLtv);
            }
            let item = rest
                .get(..usize::from(length))
                .ok_or(AudioError::MalformedLtv)?;
            let kind = item[0];
            if !(1..=5).contains(&kind) {
                return Err(AudioError::UnsupportedType(kind));
            }
            let index = usize::from(kind - 1);
            if fields[index].is_some() {
                return Err(AudioError::DuplicateType(kind));
            }
            if item.len() != [2, 2, 5, 3, 2][index] {
                return Err(AudioError::InvalidField(kind));
            }
            fields[index] = Some(&item[1..]);
            bytes = &rest[usize::from(length)..];
        }

        let frequency = SamplingFrequency::parse(fields[0].ok_or(AudioError::MissingType(1))?[0])?;
        let duration = match fields[1].ok_or(AudioError::MissingType(2))?[0] {
            0 => FrameDuration::Ms7_5,
            1 => FrameDuration::Ms10,
            _ => return Err(AudioError::InvalidField(2)),
        };
        let octets = fields[3].ok_or(AudioError::MissingType(4))?;
        let config = Self {
            frequency,
            duration,
            channel_allocation: fields[2]
                .map_or(0, |v| u32::from_le_bytes([v[0], v[1], v[2], v[3]])),
            octets_per_frame: u16::from_le_bytes([octets[0], octets[1]]),
            frame_blocks_per_sdu: fields[4].map_or(1, |v| v[0]),
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), AudioError> {
        if self.channel_allocation & 0xf000_0000 != 0 {
            return Err(AudioError::InvalidField(3));
        }
        // ponytail: structural bounds only; codec bitrate support needs peer capabilities.
        if self.octets_per_frame == 0 {
            return Err(AudioError::InvalidField(4));
        }
        if self.frame_blocks_per_sdu == 0 {
            return Err(AudioError::InvalidField(5));
        }
        Ok(())
    }

    /// Encodes canonical LTVs; optional default values are omitted.
    ///
    /// # Errors
    /// Returns an error for zero frame sizes/counts or reserved location bits.
    pub fn encode(&self) -> Result<Vec<u8>, AudioError> {
        self.validate()?;
        let mut bytes = vec![2, 1, self.frequency as u8, 2, 2, self.duration as u8];
        if self.channel_allocation != 0 {
            bytes.extend_from_slice(&[5, 3]);
            bytes.extend_from_slice(&self.channel_allocation.to_le_bytes());
        }
        bytes.extend_from_slice(&[3, 4]);
        bytes.extend_from_slice(&self.octets_per_frame.to_le_bytes());
        if self.frame_blocks_per_sdu != 1 {
            bytes.extend_from_slice(&[2, 5, self.frame_blocks_per_sdu]);
        }
        Ok(bytes)
    }

    /// Returns channel count, treating unspecified allocation as mono.
    ///
    /// # Errors
    /// Returns an error for invalid configuration fields.
    pub fn channel_count(&self) -> Result<u32, AudioError> {
        self.validate()?;
        Ok(self.channel_allocation.count_ones().max(1))
    }

    /// Returns encoded SDU size, checking the HCI ISO 12-bit SDU ceiling.
    ///
    /// # Errors
    /// Returns an error for invalid fields or an SDU larger than 4095 bytes.
    pub fn sdu_length(&self) -> Result<usize, AudioError> {
        let channels = self.channel_count()?;
        let size =
            channels * u32::from(self.octets_per_frame) * u32::from(self.frame_blocks_per_sdu);
        let size = usize::try_from(size).map_err(|_| AudioError::SduTooLarge)?;
        if size > MAX_SDU_LENGTH {
            return Err(AudioError::SduTooLarge);
        }
        Ok(size)
    }
}

/// Unicast BAP `QoS` field bounds and LC3 SDU capacity check.
///
/// Does not negotiate peer capabilities, latency, presentation delay, or radio
/// scheduling. Interval is explicit: e.g. 44.1 kHz/10 ms/one block uses 10884 us
/// in BAP presets, not 10000 us. Timing compatibility remains caller's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnicastQos {
    /// SDU interval in microseconds (255..=1048575).
    pub sdu_interval_us: u32,
    /// Framing assigned number: 0 unframed, 1 framed.
    pub framing: u8,
    /// Preferred PHY: 1 LE 1M, 2 LE 2M, 4 LE Coded.
    pub phy: u8,
    /// Maximum SDU size (1..=4095).
    pub max_sdu: u16,
    /// Maximum transport latency in milliseconds (5..=4000).
    pub max_transport_latency_ms: u16,
    /// Presentation delay in microseconds (24-bit unsigned).
    pub presentation_delay_us: u32,
}

impl UnicastQos {
    /// Checks field ranges and enough SDU capacity for the codec configuration.
    ///
    /// # Errors
    /// Returns the invalid `QoS` field or a codec/SDU capacity error.
    pub fn validate_for(&self, config: &Lc3Config) -> Result<(), AudioError> {
        if !(0x0000_00ff..=0x000f_ffff).contains(&self.sdu_interval_us) {
            return Err(AudioError::InvalidQos("SDU interval"));
        }
        if self.framing > 1 {
            return Err(AudioError::InvalidQos("framing"));
        }
        if !matches!(self.phy, 1 | 2 | 4) {
            return Err(AudioError::InvalidQos("PHY"));
        }
        if self.max_sdu == 0 || usize::from(self.max_sdu) > MAX_SDU_LENGTH {
            return Err(AudioError::InvalidQos("maximum SDU"));
        }
        if !(5..=4000).contains(&self.max_transport_latency_ms) {
            return Err(AudioError::InvalidQos("maximum transport latency"));
        }
        if self.presentation_delay_us > 0x00ff_ffff {
            return Err(AudioError::InvalidQos("presentation delay"));
        }
        if config.sdu_length()? > usize::from(self.max_sdu) {
            return Err(AudioError::InvalidQos("insufficient SDU capacity"));
        }
        Ok(())
    }
}

/// Configuration parsing, encoding, or sizing error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioError {
    /// LTV length is zero or exceeds available bytes.
    MalformedLtv,
    /// Required LTV type is absent.
    MissingType(u8),
    /// Type appears more than once.
    DuplicateType(u8),
    /// Type is outside this parser's supported set.
    UnsupportedType(u8),
    /// Sample rate is outside the implemented BAP LC3 set.
    UnsupportedFrequency(u8),
    /// Known LTV type has invalid length or value.
    InvalidField(u8),
    /// Encoded SDU would exceed HCI ISO size limits.
    SduTooLarge,
    /// Named `QoS` field is invalid for this software check.
    InvalidQos(&'static str),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedLtv => f.write_str("malformed LC3 configuration LTV"),
            Self::MissingType(kind) => write!(f, "missing LC3 configuration type {kind:#04x}"),
            Self::DuplicateType(kind) => write!(f, "duplicate LC3 configuration type {kind:#04x}"),
            Self::UnsupportedType(kind) => {
                write!(f, "unsupported LC3 configuration type {kind:#04x}")
            }
            Self::UnsupportedFrequency(value) => {
                write!(f, "unsupported LC3 frequency {value:#04x}")
            }
            Self::InvalidField(kind) => write!(f, "invalid LC3 configuration type {kind:#04x}"),
            Self::SduTooLarge => f.write_str("LC3 SDU exceeds HCI ISO length limit"),
            Self::InvalidQos(field) => write!(f, "invalid unicast QoS: {field}"),
        }
    }
}

impl core::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;

    // BlueZ 5.83 src/shared/lc3.h LC3_CONFIG_16_2, 16 kHz/10 ms/40 octets.
    const MONO: &[u8] = &[2, 1, 3, 2, 2, 1, 3, 4, 40, 0];

    fn qos() -> UnicastQos {
        UnicastQos {
            sdu_interval_us: 10000,
            framing: 0,
            phy: 2,
            max_sdu: 40,
            max_transport_latency_ms: 20,
            presentation_delay_us: 40000,
        }
    }

    #[test]
    fn bluez_mono_vector_has_defaults_and_round_trips() {
        let config = Lc3Config::parse(MONO).unwrap();
        assert_eq!(config.frequency, SamplingFrequency::Hz16000);
        assert_eq!(config.duration, FrameDuration::Ms10);
        assert_eq!(config.channel_count().unwrap(), 1);
        assert_eq!(config.frame_blocks_per_sdu, 1);
        assert_eq!(config.sdu_length().unwrap(), 40);
        assert_eq!(config.encode().unwrap(), MONO);
        assert_eq!(qos().validate_for(&config), Ok(()));
    }

    #[test]
    fn stereo_multiple_blocks_use_one_frame_per_channel_per_block() {
        let wire = [2, 1, 8, 2, 2, 1, 5, 3, 3, 0, 0, 0, 3, 4, 120, 0, 2, 5, 2];
        let config = Lc3Config::parse(&wire).unwrap();
        assert_eq!(config.channel_count().unwrap(), 2);
        assert_eq!(config.sdu_length().unwrap(), 480);
        assert_eq!(config.encode().unwrap(), wire);
    }

    #[test]
    fn parse_rejects_truncated_zero_duplicate_and_unknown_ltvs() {
        for end in 0..MONO.len() {
            assert!(Lc3Config::parse(&MONO[..end]).is_err(), "prefix {end}");
        }
        for (suffix, error) in [
            (&[0][..], AudioError::MalformedLtv),
            (&[2, 5][..], AudioError::MalformedLtv),
            (&[2, 1, 3][..], AudioError::DuplicateType(1)),
            (&[1, 0xff][..], AudioError::UnsupportedType(0xff)),
            (&[1, 5][..], AudioError::InvalidField(5)),
            (&[2, 5, 0][..], AudioError::InvalidField(5)),
            (&[5, 3, 0, 0, 0, 0x10][..], AudioError::InvalidField(3)),
        ] {
            let mut bytes = MONO.to_vec();
            bytes.extend_from_slice(suffix);
            assert_eq!(Lc3Config::parse(&bytes), Err(error));
        }
    }

    #[test]
    fn parse_rejects_reserved_frequency_duration_and_zero_frame_size() {
        for (offset, value, error) in [
            (2, 0, AudioError::UnsupportedFrequency(0)),
            (5, 2, AudioError::InvalidField(2)),
            (8, 0, AudioError::InvalidField(4)),
        ] {
            let mut wire = MONO.to_vec();
            wire[offset] = value;
            assert_eq!(Lc3Config::parse(&wire), Err(error));
        }
    }

    #[test]
    fn optional_defaults_and_arbitrary_order_encode_canonically() {
        let wire = [3, 4, 40, 0, 2, 5, 1, 2, 2, 1, 5, 3, 0, 0, 0, 0, 2, 1, 3];
        assert_eq!(Lc3Config::parse(&wire).unwrap().encode().unwrap(), MONO);
    }

    #[test]
    fn sizing_enforces_hci_ceiling_without_multiplication_overflow() {
        let mut config = Lc3Config::parse(MONO).unwrap();
        config.octets_per_frame = 4095;
        assert_eq!(config.sdu_length(), Ok(MAX_SDU_LENGTH));
        config.octets_per_frame = 4096;
        assert_eq!(config.sdu_length(), Err(AudioError::SduTooLarge));
        config.channel_allocation = 0x0fff_ffff;
        config.octets_per_frame = u16::MAX;
        config.frame_blocks_per_sdu = u8::MAX;
        assert_eq!(config.sdu_length(), Err(AudioError::SduTooLarge));
    }

    #[test]
    fn qos_rejects_invalid_fields_and_small_sdu_budget() {
        let config = Lc3Config::parse(MONO).unwrap();
        let valid = qos();
        for invalid in [
            UnicastQos {
                sdu_interval_us: 254,
                ..valid
            },
            UnicastQos {
                sdu_interval_us: 0x10_0000,
                ..valid
            },
            UnicastQos {
                framing: 2,
                ..valid
            },
            UnicastQos { phy: 0, ..valid },
            UnicastQos { phy: 3, ..valid },
            UnicastQos {
                max_sdu: 4096,
                ..valid
            },
            UnicastQos {
                max_sdu: 39,
                ..valid
            },
            UnicastQos {
                max_transport_latency_ms: 4,
                ..valid
            },
            UnicastQos {
                max_transport_latency_ms: 4001,
                ..valid
            },
            UnicastQos {
                presentation_delay_us: 0x0100_0000,
                ..valid
            },
        ] {
            assert!(invalid.validate_for(&config).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn qos_accepts_44100_preset_without_assuming_nominal_interval() {
        // Zephyr BAP 441_2_1: framed, 10884 us interval, 130 octets/frame.
        let config = Lc3Config::parse(&[2, 1, 7, 2, 2, 1, 3, 4, 130, 0]).unwrap();
        let qos = UnicastQos {
            sdu_interval_us: 10884,
            framing: 1,
            max_sdu: 130,
            max_transport_latency_ms: 31,
            ..qos()
        };
        assert_eq!(qos.validate_for(&config), Ok(()));
    }
}
