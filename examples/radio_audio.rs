//! Small LC3 fixture for raw-radio experiments, not a BIS implementation.
//! Generate: `cargo run --features lc3 --example radio_audio -- generate frames.lc3`
//! Native HCI fixture: ... -- generate-native twenty-frames.lc3
//! Decode ordered, deduplicated SDUs: ... -- decode frames.lc3 decoded.s16le

use cyw43_controller_proto::ltv::Lc3Config;
use cyw43_iso_re::lc3::{Complex, Decoder, Encoder};
use std::{error::Error, fs};

const FRAME_BYTES: usize = 26;
const FRAME_SAMPLES: usize = 80;

fn config() -> Result<Lc3Config, Box<dyn Error>> {
    Ok(Lc3Config::parse(&[2, 1, 1, 2, 2, 1, 3, 4, 26, 0])?)
}

fn generate(count: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let config = config()?;
    let (i, s, c) = Encoder::<1>::working_lengths(config)?;
    let (mut integers, mut scalars, mut complex) =
        (vec![0; i], vec![0.0; s], vec![Complex::default(); c]);
    let mut encoder = Encoder::<1>::new(config, &mut integers, &mut scalars, &mut complex)?;
    let mut frames = vec![0; count * FRAME_BYTES];
    for (index, output) in frames
        .as_chunks_mut::<FRAME_BYTES>()
        .0
        .iter_mut()
        .enumerate()
    {
        let mut pcm = [0; FRAME_SAMPLES];
        let amplitude = 6000 + i16::try_from(index / 4)? * 500;
        for (sample, value) in pcm.iter_mut().enumerate() {
            *value = if sample % (16 >> (index % 4)) < (8 >> (index % 4)) {
                amplitude
            } else {
                -amplitude
            };
        }
        encoder.encode_sdu(&pcm, output)?;
    }
    Ok(frames)
}

fn decode(frames: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    if frames.is_empty() || !frames.len().is_multiple_of(FRAME_BYTES) {
        return Err("expected nonempty concatenated 26-byte LC3 SDUs".into());
    }
    let config = config()?;
    let (s, c) = Decoder::<1>::working_lengths(config)?;
    let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
    let mut decoder = Decoder::<1>::new(config, &mut scalars, &mut complex)?;
    let mut output = Vec::new();
    for frame in frames.as_chunks::<FRAME_BYTES>().0 {
        let mut pcm = [0; FRAME_SAMPLES];
        decoder.decode_sdu(Some(frame), &mut pcm)?;
        output.extend(pcm.into_iter().flat_map(i16::to_le_bytes));
    }
    Ok(output)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [command, output] if command == "generate" || command == "generate-native" => {
            let count = if command == "generate" { 4 } else { 20 };
            let frames = generate(count)?;
            let pcm = decode(&frames)?;
            assert_eq!(pcm.len(), count * FRAME_SAMPLES * 2);
            assert!(
                pcm.as_chunks::<2>()
                    .0
                    .iter()
                    .any(|s| { i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 1000 })
            );
            assert!(
                frames
                    .as_chunks::<FRAME_BYTES>()
                    .0
                    .iter()
                    .enumerate()
                    .all(|(i, f)| {
                        frames
                            .as_chunks::<FRAME_BYTES>()
                            .0
                            .iter()
                            .skip(i + 1)
                            .all(|g| f != g)
                    })
            );
            fs::write(output, frames)?;
            println!(
                "{count} distinct LC3 SDUs; mono 8000 Hz, 10 ms, 26 bytes; local decode passes"
            );
        }
        [command, input, output] if command == "decode" => {
            fs::write(output, decode(&fs::read(input)?)?)?;
            println!("Decoded mono 8000 Hz signed 16-bit little-endian PCM");
        }
        _ => {
            return Err(
                "usage: radio_audio generate OUTPUT | generate-native OUTPUT | decode INPUT OUTPUT"
                    .into(),
            );
        }
    }
    Ok(())
}
