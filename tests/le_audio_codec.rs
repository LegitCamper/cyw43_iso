#![cfg(feature = "lc3")]

use cyw43_iso_re::{
    lc3::{CodecError, Complex, Decoder, Encoder, Layout},
    le_audio::{FrameDuration, Lc3Config, SamplingFrequency},
};

fn mono() -> Lc3Config {
    Lc3Config::parse(&[2, 1, 3, 2, 2, 1, 3, 4, 40, 0]).unwrap()
}

#[test]
fn streaming_tone_survives_codec_with_useful_signal_quality_and_loss_concealment() {
    let config = mono();
    let layout = Layout::new(config).unwrap();
    let (i, s, c) = Encoder::<1>::working_lengths(config).unwrap();
    let (mut integers, mut scalars, mut complex) =
        (vec![0; i], vec![0.0; s], vec![Complex::default(); c]);
    let mut encoder = Encoder::<1>::new(config, &mut integers, &mut scalars, &mut complex).unwrap();
    let (s, c) = Decoder::<1>::working_lengths(config).unwrap();
    let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
    let mut decoder = Decoder::<1>::new(config, &mut scalars, &mut complex).unwrap();
    let mut wire = vec![0; layout.sdu_bytes()];
    let mut output_pcm = vec![0; layout.pcm_samples()];
    let source: Vec<i16> = (0..160)
        .map(|i| {
            #[allow(clippy::cast_possible_truncation)]
            let sample = (10000.0 * (f64::from(i) * core::f64::consts::TAU / 16.0).sin()) as i16;
            sample
        })
        .collect();
    for _ in 0..8 {
        encoder.encode_sdu(&source, &mut wire).unwrap();
        decoder.decode_sdu(Some(&wire), &mut output_pcm).unwrap();
    }
    // Search one period to account for the codec's algorithmic delay.
    let error = (0..16)
        .map(|delay| {
            output_pcm
                .iter()
                .enumerate()
                .map(|(i, sample)| {
                    (f64::from(*sample) - f64::from(source[(i + delay) % source.len()])).powi(2)
                })
                .sum::<f64>()
        })
        .fold(f64::INFINITY, f64::min);
    let energy: f64 = source.iter().map(|sample| f64::from(*sample).powi(2)).sum();
    assert!(10.0 * (energy / error).log10() > 20.0);
    decoder.decode_sdu(None, &mut output_pcm).unwrap();
    assert!(output_pcm.iter().any(|sample| *sample != 0));
}

#[test]
fn all_rates_and_durations_encode_decode_silence() {
    for frequency in [
        SamplingFrequency::Hz8000,
        SamplingFrequency::Hz16000,
        SamplingFrequency::Hz24000,
        SamplingFrequency::Hz32000,
        SamplingFrequency::Hz44100,
        SamplingFrequency::Hz48000,
    ] {
        for duration in [FrameDuration::Ms7_5, FrameDuration::Ms10] {
            let config = Lc3Config {
                frequency,
                duration,
                octets_per_frame: 100,
                ..mono()
            };
            let layout = Layout::new(config).unwrap();
            let (i, s, c) = Encoder::<1>::working_lengths(config).unwrap();
            let (mut integers, mut scalars, mut complex) =
                (vec![0; i], vec![0.0; s], vec![Complex::default(); c]);
            let mut encoder =
                Encoder::<1>::new(config, &mut integers, &mut scalars, &mut complex).unwrap();
            let (s, c) = Decoder::<1>::working_lengths(config).unwrap();
            let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
            let mut decoder = Decoder::<1>::new(config, &mut scalars, &mut complex).unwrap();
            let pcm = vec![0; layout.pcm_samples()];
            let mut wire = vec![0; layout.sdu_bytes()];
            let mut output_pcm = vec![123; layout.pcm_samples()];
            encoder.encode_sdu(&pcm, &mut wire).unwrap();
            decoder.decode_sdu(Some(&wire), &mut output_pcm).unwrap();
            assert!(
                output_pcm.iter().all(|sample| sample.abs() <= 1),
                "{frequency:?}/{duration:?}"
            );
        }
    }
}

#[test]
fn stereo_blocks_match_separate_mono_streams() {
    let config = Lc3Config {
        channel_allocation: 3,
        frame_blocks_per_sdu: 2,
        ..mono()
    };
    let layout = Layout::new(config).unwrap();
    let (i, s, c) = Encoder::<2>::working_lengths(config).unwrap();
    let (mut integers, mut scalars, mut complex) =
        (vec![0; i], vec![0.0; s], vec![Complex::default(); c]);
    let mut encoder = Encoder::<2>::new(config, &mut integers, &mut scalars, &mut complex).unwrap();
    let pcm: Vec<i16> = (0..layout.pcm_samples())
        .map(|i| i16::try_from((i * 97) % 10000).unwrap())
        .collect();
    let mut wire = vec![0; layout.sdu_bytes()];
    encoder.encode_sdu(&pcm, &mut wire).unwrap();
    for channel in 0..2 {
        let (i, s, c) = Encoder::<1>::working_lengths(mono()).unwrap();
        let (mut integers, mut scalars, mut complex) =
            (vec![0; i], vec![0.0; s], vec![Complex::default(); c]);
        let mut encoder =
            Encoder::<1>::new(mono(), &mut integers, &mut scalars, &mut complex).unwrap();
        for block in 0..2 {
            let index = block * 2 + channel;
            let mut expected = [0; 40];
            encoder
                .encode_sdu(&pcm[index * 160..(index + 1) * 160], &mut expected)
                .unwrap();
            assert_eq!(&wire[index * 40..(index + 1) * 40], &expected);
        }
    }
}

#[test]
fn invalid_configuration_and_buffers_fail_before_backend_panics() {
    assert_eq!(
        Encoder::<2>::working_lengths(mono()),
        Err(CodecError::ChannelCount)
    );
    assert_eq!(
        Encoder::<0>::working_lengths(mono()),
        Err(CodecError::ChannelCount)
    );
    assert!(matches!(
        Encoder::<1>::new(mono(), &mut [], &mut [], &mut []),
        Err(CodecError::WorkingBuffer)
    ));
    for octets_per_frame in [0, 19, 401, u16::MAX] {
        assert!(
            Layout::new(Lc3Config {
                octets_per_frame,
                ..mono()
            })
            .is_err()
        );
    }
    let (s, c) = Decoder::<1>::working_lengths(mono()).unwrap();
    let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
    let mut decoder = Decoder::<1>::new(mono(), &mut scalars, &mut complex).unwrap();
    assert_eq!(
        decoder.decode_sdu(Some(&[0; 39]), &mut [0; 160]),
        Err(CodecError::SduLength)
    );
    assert_eq!(
        decoder.decode_sdu(None, &mut [0; 159]),
        Err(CodecError::SduLength)
    );
}

#[test]
fn malformed_frames_do_not_panic_decoder() {
    let mut seed = 42_u32;
    for frequency in [
        SamplingFrequency::Hz8000,
        SamplingFrequency::Hz16000,
        SamplingFrequency::Hz24000,
        SamplingFrequency::Hz32000,
        SamplingFrequency::Hz44100,
        SamplingFrequency::Hz48000,
    ] {
        for duration in [FrameDuration::Ms7_5, FrameDuration::Ms10] {
            for octets_per_frame in [20, 40, 100, 400] {
                let config = Lc3Config {
                    frequency,
                    duration,
                    octets_per_frame,
                    ..mono()
                };
                let (s, c) = Decoder::<1>::working_lengths(config).unwrap();
                let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
                let mut decoder = Decoder::<1>::new(config, &mut scalars, &mut complex).unwrap();
                let mut pcm = vec![0; Layout::new(config).unwrap().pcm_samples()];
                for iteration in 0..200 {
                    let mut frame = vec![0; usize::from(octets_per_frame)];
                    for byte in &mut frame {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        *byte = match iteration {
                            0 => 0,
                            1 => 0xff,
                            _ => seed.to_le_bytes()[0],
                        };
                    }
                    decoder.decode_sdu(Some(&frame), &mut pcm).unwrap();
                }
            }
        }
    }
}

#[test]
fn decoder_agrees_with_independent_google_lc3_reference() {
    let wire = include_bytes!("fixtures/lc3/google-16k-10ms-40b.lc3");
    let reference = include_bytes!("fixtures/lc3/google-16k-final-frame.pcm");
    let (s, c) = Decoder::<1>::working_lengths(mono()).unwrap();
    let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
    let mut decoder = Decoder::<1>::new(mono(), &mut scalars, &mut complex).unwrap();
    let mut output = [0; 160];
    for frame in wire.as_chunks::<40>().0 {
        decoder.decode_sdu(Some(frame), &mut output).unwrap();
    }
    let mut energy = 0.0_f64;
    let mut error = 0.0_f64;
    for (sample, expected) in output.iter().zip(reference.as_chunks::<2>().0) {
        let expected = f64::from(i16::from_le_bytes(*expected));
        energy += expected.powi(2);
        error += (f64::from(*sample) - expected).powi(2);
    }
    assert!(10.0 * (energy / error).log10() > 45.0);
}

#[test]
fn backend_tail_reader_rejects_empty_and_exhausted_input() {
    use lc3_codec::decoder::buffer_reader::BufferReader;
    let mut reader = BufferReader::new();
    assert!(reader.read_tail_bool(&[]).is_err());
    for _ in 0..8 {
        assert!(reader.read_tail_bool(&[0xff]).unwrap());
    }
    assert!(reader.read_tail_bool(&[0xff]).is_err());
    assert_eq!(reader.get_tail_bit_cursor(), 8);
    // Arithmetic head read-ahead can overlap valid tail bits.
    assert!(BufferReader::new_at(3, 0).read_tail_bool(&[0xff]).unwrap());
}
