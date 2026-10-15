#![no_std]
#![no_main]

use embassy_time::Instant;
use lc3_codec::{
    common::{
        complex::Complex,
        config::{FrameDuration, SamplingFrequency},
    },
    decoder::lc3_decoder::Lc3Decoder,
    encoder::lc3_encoder::Lc3Encoder,
};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

const ENCODER: (usize, usize, usize) =
    Lc3Encoder::<1>::calc_working_buffer_lengths(FrameDuration::TenMs, SamplingFrequency::Hz8000);
const DECODER: (usize, usize) =
    Lc3Decoder::<1>::calc_working_buffer_lengths(FrameDuration::TenMs, SamplingFrequency::Hz8000);
const FRAMES: usize = 20;
const DEADLINE_US: u64 = 10_000;

struct Workspace {
    encoder_integer: [i16; ENCODER.0],
    encoder_scalar: [f32; ENCODER.1],
    encoder_complex: [Complex; ENCODER.2],
    decoder_scalar: [f32; DECODER.0],
    decoder_complex: [Complex; DECODER.1],
}

static WORK: StaticCell<Workspace> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let _peripherals = embassy_rp::init(Default::default());
    defmt::info!(
        "LC3_BENCH start clock_hz={} rate=8000 samples=80 octets=26 frames={} deadline_us={} workspace_bytes={} rom_float={}",
        embassy_rp::clocks::clk_sys_freq(),
        FRAMES,
        DEADLINE_US,
        core::mem::size_of::<Workspace>(),
        cfg!(feature = "rom-float")
    );
    let work = WORK.init(Workspace {
        encoder_integer: [0; ENCODER.0],
        encoder_scalar: [0.0; ENCODER.1],
        encoder_complex: [Complex { r: 0.0, i: 0.0 }; ENCODER.2],
        decoder_scalar: [0.0; DECODER.0],
        decoder_complex: [Complex { r: 0.0, i: 0.0 }; DECODER.1],
    });
    let mut encoder = Lc3Encoder::<1>::new(
        FrameDuration::TenMs,
        SamplingFrequency::Hz8000,
        &mut work.encoder_integer,
        &mut work.encoder_scalar,
        &mut work.encoder_complex,
    );
    let mut decoder = Lc3Decoder::<1>::new(
        FrameDuration::TenMs,
        SamplingFrequency::Hz8000,
        &mut work.decoder_scalar,
        &mut work.decoder_complex,
    );
    let mut timing = [[0_u64; 2]; FRAMES];
    let mut encoded = [[0_u8; 26]; FRAMES];
    let mut peak = 0_u16;
    let mut nonzero = 0_u32;
    let mut digest = 0x811c_9dc5_u32;
    let timer_start = Instant::now();
    let timer_overhead_us = timer_start.elapsed().as_micros();
    for index in 0..FRAMES {
        let mut input = [0_i16; 80];
        let mut output = [0_i16; 80];
        let amplitude = 6000 + (index / 4) as i16 * 500;
        for (sample, value) in input.iter_mut().enumerate() {
            *value = if sample % (16 >> (index % 4)) < (8 >> (index % 4)) {
                amplitude
            } else {
                -amplitude
            };
        }
        let start = Instant::now();
        encoder
            .encode_frame(0, &input, &mut encoded[index])
            .expect("encode");
        timing[index][0] = start.elapsed().as_micros();
        let start = Instant::now();
        decoder
            .decode_frame(16, 0, &encoded[index], &mut output)
            .expect("decode");
        timing[index][1] = start.elapsed().as_micros();
        for sample in output {
            peak = peak.max(sample.unsigned_abs());
            nonzero += u32::from(sample != 0);
        }
        for byte in encoded[index] {
            digest = (digest ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
    }
    // ponytail: twenty fixture frames, not a worst-case or concurrent radio load.
    let mut plc_output = [0_i16; 80];
    let start = Instant::now();
    decoder
        .decode_frame(16, 0, &[], &mut plc_output)
        .expect("PLC");
    let plc_us = start.elapsed().as_micros();
    assert!(peak > 1000 && nonzero > 0, "decoded audio must be nonzero");
    for (index, frame) in encoded.iter().enumerate() {
        assert!(
            !encoded[..index].contains(frame),
            "fixture SDUs must differ"
        );
    }
    let mut encode_max = 0;
    let mut decode_max = 0;
    let mut combined_max = 0;
    let mut encode_total = 0;
    let mut decode_total = 0;
    let mut encode_misses = 0;
    let mut decode_misses = 0;
    let mut combined_misses = 0;
    for (index, [encode, decode]) in timing.into_iter().enumerate() {
        encode_max = encode_max.max(encode);
        decode_max = decode_max.max(decode);
        combined_max = combined_max.max(encode + decode);
        encode_total += encode;
        decode_total += decode;
        encode_misses += usize::from(encode > DEADLINE_US);
        decode_misses += usize::from(decode > DEADLINE_US);
        combined_misses += usize::from(encode + decode > DEADLINE_US);
        defmt::info!(
            "LC3_BENCH frame={} encode_us={} decode_us={} data={:02x}",
            index,
            encode,
            decode,
            encoded[index]
        );
    }
    defmt::info!(
        "LC3_BENCH result frames={} encode_max_us={} decode_max_us={} combined_max_us={} encode_misses={} decode_misses={} combined_misses={} plc_us={} timer_overhead_us={} peak={} nonzero_samples={} fixture_fnv32={=u32:08x} encode_total_us={} decode_total_us={}",
        FRAMES,
        encode_max,
        decode_max,
        combined_max,
        encode_misses,
        decode_misses,
        combined_misses,
        plc_us,
        timer_overhead_us,
        peak,
        nonzero,
        digest,
        encode_total,
        decode_total
    );
    defmt::info!("LC3_BENCH done");
    loop {
        cortex_m::asm::wfi();
    }
}
