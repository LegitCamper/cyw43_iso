//! Proves an allocation-using profile codec and our fixed-channel codec coexist.

use cyw43_iso_re::{
    lc3::{Complex, Decoder, Encoder},
    le_audio::Lc3Config,
};
use lc3_codec::{
    common::{
        complex::Complex as AllocComplex,
        config::{FrameDuration, SamplingFrequency},
    },
    decoder::lc3_decoder::Lc3Decoder,
    encoder::lc3_encoder::Lc3Encoder,
};

fn main() {
    let config = Lc3Config::parse(&[2, 1, 3, 2, 2, 1, 3, 4, 40, 0]).unwrap();
    let duration = FrameDuration::TenMs;
    let frequency = SamplingFrequency::Hz16000;

    let (i, s, c) = Encoder::<1>::working_lengths(config).unwrap();
    let (mut integers, mut scalars, mut complex) =
        (vec![0; i], vec![0.0; s], vec![Complex::default(); c]);
    let mut fixed_encoder =
        Encoder::<1>::new(config, &mut integers, &mut scalars, &mut complex).unwrap();
    let (s, c) = Lc3Decoder::calc_working_buffer_lengths(1, duration, frequency);
    let (mut alloc_scalars, mut alloc_complex) = (vec![0.0; s], vec![AllocComplex::default(); c]);
    let mut alloc_decoder = Lc3Decoder::new(
        1,
        duration,
        frequency,
        &mut alloc_scalars,
        &mut alloc_complex,
    );

    let (i, s, c) = Lc3Encoder::calc_working_buffer_lengths(1, duration, frequency);
    let (mut alloc_integers, mut alloc_scalars, mut alloc_complex) =
        (vec![0; i], vec![0.0; s], vec![AllocComplex::default(); c]);
    let mut alloc_encoder = Lc3Encoder::new(
        1,
        duration,
        frequency,
        &mut alloc_integers,
        &mut alloc_scalars,
        &mut alloc_complex,
    );
    let (s, c) = Decoder::<1>::working_lengths(config).unwrap();
    let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
    let mut fixed_decoder = Decoder::<1>::new(config, &mut scalars, &mut complex).unwrap();

    let pcm: Vec<i16> = (0..160)
        .map(|index| if index % 16 < 8 { 5000 } else { -5000 })
        .collect();
    let mut fixed_wire = [0; 40];
    let mut alloc_wire = [0; 40];
    let mut fixed_pcm = [0; 160];
    let mut alloc_pcm = [0; 160];
    for _ in 0..8 {
        fixed_encoder.encode_sdu(&pcm, &mut fixed_wire).unwrap();
        alloc_decoder
            .decode_frame(16, 0, &fixed_wire, &mut alloc_pcm)
            .unwrap();
        alloc_encoder
            .encode_frame(0, &pcm, &mut alloc_wire)
            .unwrap();
        fixed_decoder
            .decode_sdu(Some(&alloc_wire), &mut fixed_pcm)
            .unwrap();
        assert_eq!(fixed_wire, alloc_wire);
        assert_eq!(fixed_pcm, alloc_pcm);
    }
    assert!(fixed_pcm.iter().any(|sample| sample.unsigned_abs() > 1000));
    println!("Both LC3 packages coexist; bidirectional codec check passed.");
}
