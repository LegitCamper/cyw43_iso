//! Allocation-free LC3 SDU codec using `lc3-codec` 0.2.
//!
//! PCM layout is block, channel, sample (planar within each block); compressed
//! layout is block, channel, octet, as required by BAP. Channel order follows
//! ascending Audio Location bits. Caller-owned working buffers retain codec
//! history across SDUs. This backend is not Bluetooth qualified.

use crate::le_audio::{AudioError, FrameDuration, Lc3Config, SamplingFrequency};
use core::fmt;
pub use lc3_codec::common::complex::Complex;
use lc3_codec::{
    common::config::{
        FrameDuration as Duration, Lc3Config as Config, SamplingFrequency as Frequency,
    },
    decoder::lc3_decoder::Lc3Decoder,
    encoder::lc3_encoder::Lc3Encoder,
};

/// Checked dimensions shared by encoder and decoder.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    config: Lc3Config,
    channels: usize,
    samples_per_frame: usize,
    sdu_bytes: usize,
}

impl Layout {
    /// Validates LC3 frame size (20..=400 octets) and HCI SDU bounds.
    ///
    /// # Errors
    /// Returns invalid codec fields or unsupported frame size.
    pub fn new(config: Lc3Config) -> Result<Self, CodecError> {
        let sdu_bytes = config.sdu_length().map_err(CodecError::Config)?;
        if !(20..=400).contains(&config.octets_per_frame) {
            return Err(CodecError::FrameSize);
        }
        let channels = usize::try_from(config.channel_count().map_err(CodecError::Config)?)
            .map_err(|_| CodecError::ChannelCount)?;
        let (duration, frequency) = backend_config(config);
        Ok(Self {
            config,
            channels,
            samples_per_frame: Config::new(frequency, duration).nf,
            sdu_bytes,
        })
    }

    /// PCM samples per channel per codec frame (480 for 44.1 kHz/10 ms).
    #[must_use]
    pub const fn samples_per_frame(self) -> usize {
        self.samples_per_frame
    }

    /// Total planar PCM samples in one SDU.
    #[must_use]
    pub fn pcm_samples(self) -> usize {
        self.samples_per_frame * self.channels * usize::from(self.config.frame_blocks_per_sdu)
    }

    /// Encoded bytes in one SDU.
    #[must_use]
    pub const fn sdu_bytes(self) -> usize {
        self.sdu_bytes
    }

    fn check_channels<const CHANNELS: usize>(self) -> Result<(), CodecError> {
        if self.channels == CHANNELS {
            Ok(())
        } else {
            Err(CodecError::ChannelCount)
        }
    }
}

fn backend_config(config: Lc3Config) -> (Duration, Frequency) {
    let duration = match config.duration {
        FrameDuration::Ms7_5 => Duration::SevenPointFiveMs,
        FrameDuration::Ms10 => Duration::TenMs,
    };
    let frequency = match config.frequency {
        SamplingFrequency::Hz8000 => Frequency::Hz8000,
        SamplingFrequency::Hz16000 => Frequency::Hz16000,
        SamplingFrequency::Hz24000 => Frequency::Hz24000,
        SamplingFrequency::Hz32000 => Frequency::Hz32000,
        SamplingFrequency::Hz44100 => Frequency::Hz44100,
        SamplingFrequency::Hz48000 => Frequency::Hz48000,
    };
    (duration, frequency)
}

/// Stateful LC3 encoder. No allocation occurs during construction or encoding.
pub struct Encoder<'a, const CHANNELS: usize = 1> {
    layout: Layout,
    codec: Lc3Encoder<'a, CHANNELS>,
}

impl<'a, const CHANNELS: usize> Encoder<'a, CHANNELS> {
    /// Returns required i16, f32, and complex working-buffer lengths.
    ///
    /// # Errors
    /// Returns invalid configuration or mismatched channel count.
    pub fn working_lengths(config: Lc3Config) -> Result<(usize, usize, usize), CodecError> {
        Layout::new(config)?.check_channels::<CHANNELS>()?;
        let (duration, frequency) = backend_config(config);
        Ok(Lc3Encoder::<CHANNELS>::calc_working_buffer_lengths(
            duration, frequency,
        ))
    }

    /// Borrows working buffers for the lifetime of the stream.
    ///
    /// # Errors
    /// Returns invalid configuration, channel count, or undersized buffers.
    pub fn new(
        config: Lc3Config,
        integers: &'a mut [i16],
        scalars: &'a mut [f32],
        complex: &'a mut [Complex],
    ) -> Result<Self, CodecError> {
        let (i, s, c) = Self::working_lengths(config)?;
        if integers.len() < i || scalars.len() < s || complex.len() < c {
            return Err(CodecError::WorkingBuffer);
        }
        let (duration, frequency) = backend_config(config);
        Ok(Self {
            layout: Layout::new(config)?,
            codec: Lc3Encoder::new(duration, frequency, integers, scalars, complex),
        })
    }

    /// Encodes exactly one SDU; lengths are checked before codec state changes.
    ///
    /// # Errors
    /// Returns wrong PCM or SDU buffer length.
    pub fn encode_sdu(&mut self, pcm: &[i16], output: &mut [u8]) -> Result<(), CodecError> {
        if pcm.len() != self.layout.pcm_samples() || output.len() != self.layout.sdu_bytes {
            return Err(CodecError::SduLength);
        }
        for (index, (samples, frame)) in pcm
            .chunks_exact(self.layout.samples_per_frame)
            .zip(output.chunks_exact_mut(usize::from(self.layout.config.octets_per_frame)))
            .enumerate()
        {
            self.codec
                .encode_frame(index % CHANNELS, samples, frame)
                .map_err(|never| match never {})?;
        }
        Ok(())
    }
}

/// Stateful LC3 decoder with upstream packet-loss concealment.
pub struct Decoder<'a, const CHANNELS: usize = 1> {
    layout: Layout,
    codec: Lc3Decoder<'a, CHANNELS>,
}

impl<'a, const CHANNELS: usize> Decoder<'a, CHANNELS> {
    /// Returns checked PCM/SDU dimensions for validating a transport boundary.
    #[must_use]
    pub const fn layout(&self) -> Layout {
        self.layout
    }

    /// Returns required f32 and complex working-buffer lengths.
    ///
    /// # Errors
    /// Returns invalid configuration or mismatched channel count.
    pub fn working_lengths(config: Lc3Config) -> Result<(usize, usize), CodecError> {
        Layout::new(config)?.check_channels::<CHANNELS>()?;
        let (duration, frequency) = backend_config(config);
        Ok(Lc3Decoder::<CHANNELS>::calc_working_buffer_lengths(
            duration, frequency,
        ))
    }

    /// Borrows working buffers for the lifetime of the stream.
    ///
    /// # Errors
    /// Returns invalid configuration, channel count, or undersized buffers.
    pub fn new(
        config: Lc3Config,
        scalars: &'a mut [f32],
        complex: &'a mut [Complex],
    ) -> Result<Self, CodecError> {
        let (s, c) = Self::working_lengths(config)?;
        if scalars.len() < s || complex.len() < c {
            return Err(CodecError::WorkingBuffer);
        }
        let (duration, frequency) = backend_config(config);
        Ok(Self {
            layout: Layout::new(config)?,
            codec: Lc3Decoder::new(duration, frequency, scalars, complex),
        })
    }

    /// Decodes one SDU, or conceals one lost SDU when `input` is `None`.
    ///
    /// Upstream silently conceals corrupt codec frames; success does not certify
    /// bitstream validity. HCI invalid/lost status must be mapped to `None`.
    ///
    /// # Errors
    /// Returns wrong buffer lengths or an upstream decode error.
    pub fn decode_sdu(&mut self, input: Option<&[u8]>, pcm: &mut [i16]) -> Result<(), CodecError> {
        if pcm.len() != self.layout.pcm_samples()
            || input.is_some_and(|bytes| bytes.len() != self.layout.sdu_bytes)
        {
            return Err(CodecError::SduLength);
        }
        let frame_bytes = usize::from(self.layout.config.octets_per_frame);
        for (index, samples) in pcm
            .chunks_exact_mut(self.layout.samples_per_frame)
            .enumerate()
        {
            let frame = input.map_or(&[][..], |bytes| {
                &bytes[index * frame_bytes..(index + 1) * frame_bytes]
            });
            self.codec
                .decode_frame(16, index % CHANNELS, frame, samples)
                .map_err(|_| CodecError::Decode)?;
        }
        Ok(())
    }
}

/// Codec boundary validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// Invalid BAP codec configuration.
    Config(AudioError),
    /// LC3 frame must contain 20..=400 bytes.
    FrameSize,
    /// Compile-time channel count differs from Audio Location count.
    ChannelCount,
    /// Caller-provided working buffer is too short.
    WorkingBuffer,
    /// PCM or compressed buffer length differs from one configured SDU.
    SduLength,
    /// Backend failed to decode a frame.
    Decode,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LC3 codec error: {self:?}")
    }
}

impl core::error::Error for CodecError {}
