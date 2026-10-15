use cyw43_iso_re::{
    bap::AseCommand,
    broadcast::{SingleBisBase, broadcast_audio_announcement},
    lc3::{Complex, Decoder, Encoder},
    le_audio::Lc3Config,
};
use cyw43_trouble_composition::{ReceiveError, decode_received};
use trouble_audio::{
    ascs::AseControlPointOperation,
    cis::{Lc3Frame, RawLc3},
};
use trouble_host::prelude::*;

fn main() {
    let config = Lc3Config::parse(&[2, 1, 3, 2, 2, 1, 3, 4, 40, 0]).unwrap();
    let command = AseCommand::ConfigureCodec {
        target_latency: 2,
        target_phy: 2,
        config,
    }
    .encode(7)
    .unwrap();
    let operation = AseControlPointOperation::from_gatt(&command).unwrap();
    assert_eq!(operation.opcode(), 1);
    assert_eq!(operation.as_gatt(), command);

    let announcement = SingleBisBase {
        presentation_delay_us: 40000,
        config,
        streaming_contexts: 4,
    };
    let wire = announcement.periodic_advertising_data().unwrap();
    assert_eq!(
        wire,
        trouble_audio::big::basic_audio_announcement(&announcement.encode().unwrap())
    );
    let base = trouble_audio::big::parse_basic_audio_announcement(&wire)
        .unwrap()
        .unwrap();
    assert_eq!(base.presentation_delay_us, 40000);
    assert_eq!(base.subgroups.len(), 1);
    let subgroup = &base.subgroups[0];
    assert_eq!(subgroup.codec_id.to_le_bytes(), [6, 0, 0, 0, 0]);
    assert_eq!(
        Lc3Config::parse(&subgroup.codec_configuration).unwrap(),
        config
    );
    assert_eq!(subgroup.metadata, [3, 2, 4, 0]);
    assert_eq!(subgroup.bis.len(), 1);
    assert_eq!(subgroup.bis[0].index, 1);
    assert!(subgroup.bis[0].codec_configuration.is_empty());
    assert_eq!(
        broadcast_audio_announcement(0x12_3456).unwrap(),
        trouble_audio::big::broadcast_audio_announcement([0x56, 0x34, 0x12])
    );

    let (i, s, c) = Encoder::<1>::working_lengths(config).unwrap();
    let (mut integers, mut scalars, mut complex) =
        (vec![0; i], vec![0.0; s], vec![Complex::default(); c]);
    let mut encoder = Encoder::<1>::new(config, &mut integers, &mut scalars, &mut complex).unwrap();
    let (s, c) = Decoder::<1>::working_lengths(config).unwrap();
    let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
    let mut decoder = Decoder::<1>::new(config, &mut scalars, &mut complex).unwrap();
    let (mut scalars, mut complex) = (vec![0.0; s], vec![Complex::default(); c]);
    let mut reference = Decoder::<1>::new(config, &mut scalars, &mut complex).unwrap();
    let source: Vec<i16> = (0..160)
        .map(|i| if i % 16 < 8 { 5000 } else { -5000 })
        .collect();
    let mut next = None;
    let mut pcm = [0; 160];
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for index in 0..8_u16 {
        let mut wire = [0; 40];
        encoder.encode_sdu(&source, &mut wire).unwrap();
        let lost = index == 2;
        reference
            .decode_sdu((!lost).then_some(&wire), &mut pcm)
            .unwrap();
        expected.push((pcm, lost));
        if lost {
            continue;
        }
        let raw = RawLc3 {
            ase_id: 7,
            channel_allocation: None,
            sequence_number: (u16::MAX - 2).wrapping_add(index),
            timestamp_us: Some(u32::from(index) * 10000),
            frame: Lc3Frame::from_slice(&wire).unwrap(),
        };
        decode_received(
            &mut decoder,
            7,
            &raw,
            &mut next,
            &mut pcm,
            |samples, concealed| {
                actual.push((<[i16; 160]>::try_from(samples).unwrap(), concealed));
            },
        )
        .unwrap();
    }
    assert_eq!(actual, expected);
    assert_eq!(next, Some(5));
    assert!(pcm.iter().any(|sample| sample.unsigned_abs() > 1000));

    let mut raw = RawLc3 {
        ase_id: 7,
        channel_allocation: None,
        sequence_number: 4,
        timestamp_us: None,
        frame: Lc3Frame::from_slice(&[0; 40]).unwrap(),
    };
    assert!(matches!(
        decode_received(&mut decoder, 7, &raw, &mut next, &mut pcm, |_, _| panic!(
            "invalid input emitted PCM"
        )),
        Err(ReceiveError::Discontinuity { .. })
    ));
    raw.sequence_number = 10;
    assert!(matches!(
        decode_received(&mut decoder, 7, &raw, &mut next, &mut pcm, |_, _| panic!(
            "invalid input emitted PCM"
        )),
        Err(ReceiveError::Discontinuity { .. })
    ));
    raw.ase_id = 8;
    assert_eq!(
        decode_received(&mut decoder, 7, &raw, &mut next, &mut pcm, |_, _| panic!(
            "invalid input emitted PCM"
        )),
        Err(ReceiveError::WrongAse)
    );
    raw.ase_id = 7;
    raw.frame.pop();
    assert!(matches!(
        decode_received(&mut decoder, 7, &raw, &mut next, &mut pcm, |_, _| panic!(
            "invalid input emitted PCM"
        )),
        Err(ReceiveError::Codec(_))
    ));
    assert_eq!(next, Some(5));
    println!(
        "Trouble ASCS/BASE wire and RawLc3 decode passed: sequence wrap, one lost SDU, PLC, resume, invalid input."
    );
}
