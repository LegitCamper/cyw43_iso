use cyw43_controller_proto::ltv::{Lc3Config, UnicastQos};
use cyw43_iso_re::bap::{
    AseClient, AseCommand, AseDirection, AseState, AseStatus, ControlResponse, LC3_CODEC_ID,
    Lc3Capabilities, ProfileError, parse_pacs,
};

const MONO: &[u8] = &[2, 1, 3, 2, 2, 1, 3, 4, 40, 0];
const QOS_STATE: &[u8] = &[
    7, 2, 1, 2, 0x10, 0x27, 0, 0, 2, 40, 0, 3, 20, 0, 0x40, 0x9c, 0,
];
// 16/48 kHz, 7.5/10 ms, mono/stereo, 30..120 octets, up to four frames/SDU.
const CAPS: &[u8] = &[
    3, 1, 0x84, 0, 2, 2, 3, 2, 3, 3, 5, 4, 30, 0, 120, 0, 2, 5, 4,
];

#[test]
fn pacs_records_preserve_unknown_codecs_and_negotiate_lc3() {
    let mut value = vec![2];
    value.extend_from_slice(&LC3_CODEC_ID);
    value.push(u8::try_from(CAPS.len()).unwrap());
    value.extend_from_slice(CAPS);
    value.extend_from_slice(&[4, 3, 1, 4, 0]); // Preferred Audio Contexts: Media.
    value.extend_from_slice(&[0xff, 1, 0, 2, 0, 0, 0]);
    let records = parse_pacs(&value).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].codec_id, LC3_CODEC_ID);
    assert_eq!(records[1].codec_id, [0xff, 1, 0, 2, 0]);
    let caps = Lc3Capabilities::parse(records[0].capabilities).unwrap();
    let mono = Lc3Config::parse(MONO).unwrap();
    caps.supports(mono).unwrap();
    caps.supports(Lc3Config {
        channel_allocation: 3,
        frame_blocks_per_sdu: 2,
        ..mono
    })
    .unwrap();
    assert_eq!(
        caps.supports(Lc3Config {
            channel_allocation: 3,
            frame_blocks_per_sdu: 3,
            ..mono
        }),
        Err(ProfileError::UnsupportedConfiguration)
    );
    assert_eq!(
        caps.supports(Lc3Config {
            octets_per_frame: 121,
            ..mono
        }),
        Err(ProfileError::UnsupportedConfiguration)
    );
    for end in 0..value.len() {
        assert!(parse_pacs(&value[..end]).is_err(), "prefix {end}");
    }
    value.push(0);
    assert_eq!(parse_pacs(&value), Err(ProfileError::Length));
}

#[test]
fn capability_defaults_and_invalid_values() {
    let minimum = [3, 1, 4, 0, 2, 2, 2, 5, 4, 40, 0, 40, 0];
    let caps = Lc3Capabilities::parse(&minimum).unwrap();
    assert_eq!((caps.channel_counts, caps.max_frames_per_sdu), (1, 1));
    caps.supports(Lc3Config::parse(MONO).unwrap()).unwrap();
    for suffix in [&[0][..], &[2, 2, 2][..], &[2, 3, 0][..], &[2, 5, 0][..]] {
        let mut value = minimum.to_vec();
        value.extend_from_slice(suffix);
        assert!(Lc3Capabilities::parse(&value).is_err());
    }
    for duration in [0, 4, 0x12, 0x21, 0x33, 0xff] {
        let mut value = minimum;
        value[6] = duration;
        assert!(Lc3Capabilities::parse(&value).is_err());
    }
    let mut future = minimum.to_vec();
    future.extend_from_slice(&[2, 0xf0, 0x55]);
    assert_eq!(Lc3Capabilities::parse(&future).unwrap(), caps);
}

#[test]
fn codec_command_matches_ascs_layout_and_target_phy_is_not_qos_phy() {
    let config = Lc3Config::parse(MONO).unwrap();
    let mut expected = vec![1, 1, 7, 2, 3, 6, 0, 0, 0, 0, 10];
    expected.extend_from_slice(MONO);
    assert_eq!(
        AseCommand::ConfigureCodec {
            target_latency: 2,
            target_phy: 3,
            config
        }
        .encode(7)
        .unwrap(),
        expected
    );
    assert_eq!(
        AseCommand::ConfigureCodec {
            target_latency: 2,
            target_phy: 4,
            config
        }
        .encode(7),
        Err(ProfileError::Command)
    );
    assert_eq!(AseCommand::Release.encode(0), Err(ProfileError::AseId));
}

#[test]
fn qos_command_matches_24_bit_little_endian_fields() {
    let config = Lc3Config::parse(MONO).unwrap();
    let qos = UnicastQos {
        sdu_interval_us: 10000,
        framing: 0,
        phy: 2,
        max_sdu: 40,
        max_transport_latency_ms: 20,
        presentation_delay_us: 40000,
    };
    assert_eq!(
        AseCommand::ConfigureQos {
            cig_id: 1,
            cis_id: 2,
            qos,
            retransmissions: 3,
            config
        }
        .encode(7)
        .unwrap(),
        [
            2, 1, 7, 1, 2, 0x10, 0x27, 0, 0, 2, 40, 0, 3, 20, 0, 0x40, 0x9c, 0
        ]
    );
    assert!(
        AseCommand::ConfigureQos {
            cig_id: 0xf0,
            cis_id: 2,
            qos,
            retransmissions: 3,
            config
        }
        .encode(7)
        .is_err()
    );
}

#[test]
fn metadata_and_id_commands_encode_and_validate() {
    assert_eq!(
        AseCommand::Enable(&[3, 2, 4, 0]).encode(7).unwrap(),
        [3, 1, 7, 4, 3, 2, 4, 0]
    );
    assert_eq!(
        AseCommand::UpdateMetadata(&[]).encode(7).unwrap(),
        [7, 1, 7, 0]
    );
    assert_eq!(AseCommand::ReceiverStartReady.encode(7).unwrap(), [4, 1, 7]);
    assert_eq!(AseCommand::Disable.encode(7).unwrap(), [5, 1, 7]);
    assert_eq!(AseCommand::ReceiverStopReady.encode(7).unwrap(), [6, 1, 7]);
    assert_eq!(AseCommand::Release.encode(7).unwrap(), [8, 1, 7]);
    for bad in [&[0][..], &[3, 2, 4][..]] {
        assert!(AseCommand::Enable(bad).encode(7).is_err());
    }
}

#[test]
fn responses_keep_per_ase_failure_and_global_errors() {
    let response = ControlResponse::parse(&[1, 2, 7, 0, 0, 8, 7, 2]).unwrap();
    assert_eq!(
        response.entries().collect::<Vec<_>>(),
        [(7, 0, 0), (8, 7, 2)]
    );
    assert!(!response.global_error);
    assert!(
        ControlResponse::parse(&[0xff, 0xff, 0, 1, 0])
            .unwrap()
            .global_error
    );
    // Invalid ASE_ID response echoes zero supplied by a misbehaving client.
    assert!(ControlResponse::parse(&[1, 1, 0, 3, 0]).is_ok());
    for bad in [
        &[1, 0][..],
        &[1, 1, 7, 0][..],
        &[1, 0xff, 0, 0, 0][..],
        &[1, 1, 7, 2, 0][..],
        &[1, 1, 7, 15, 0][..],
    ] {
        assert!(ControlResponse::parse(bad).is_err());
    }
}

#[test]
fn ase_notifications_decode_state_specific_payloads() {
    assert_eq!(AseStatus::parse(&[7, 0]).unwrap().state, AseState::Idle);
    assert_eq!(
        AseStatus::parse(&[7, 6]).unwrap().state,
        AseState::Releasing
    );
    let mut codec = vec![
        7, 1, 1, 2, 3, 20, 0, 0x40, 0x9c, 0, 0x80, 0x38, 1, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 10,
    ];
    codec.extend_from_slice(MONO);
    let AseState::CodecConfigured {
        preferences,
        codec_id,
        configuration,
    } = AseStatus::parse(&codec).unwrap().state
    else {
        panic!("wrong state")
    };
    assert_eq!(codec_id, LC3_CODEC_ID);
    assert_eq!(preferences.presentation_delay_min_us, 40000);
    assert_eq!(preferences.presentation_delay_max_us, 80000);
    assert_eq!(
        Lc3Config::parse(configuration).unwrap(),
        Lc3Config::parse(MONO).unwrap()
    );
    let qos_wire = [
        7, 2, 1, 2, 0x10, 0x27, 0, 0, 2, 40, 0, 3, 20, 0, 0x40, 0x9c, 0,
    ];
    let AseState::QosConfigured {
        cig_id,
        cis_id,
        qos,
        retransmissions,
    } = AseStatus::parse(&qos_wire).unwrap().state
    else {
        panic!("wrong state")
    };
    assert_eq!((cig_id, cis_id, retransmissions), (1, 2, 3));
    assert_eq!(
        (qos.sdu_interval_us, qos.max_sdu, qos.presentation_delay_us),
        (10000, 40, 40000)
    );
    qos.validate_for(&Lc3Config::parse(MONO).unwrap()).unwrap();
    for state in 3..=5 {
        let value = [7, state, 1, 2, 4, 3, 2, 4, 0];
        let status = AseStatus::parse(&value).unwrap();
        let (AseState::Enabling { metadata, .. }
        | AseState::Streaming { metadata, .. }
        | AseState::Disabling { metadata, .. }) = status.state
        else {
            panic!("wrong state")
        };
        assert_eq!(metadata, [3, 2, 4, 0]);
        for end in 0..value.len() {
            assert!(AseStatus::parse(&value[..end]).is_err());
        }
    }
    for value in [&codec[..], &qos_wire[..]] {
        for end in 0..value.len() {
            assert!(AseStatus::parse(&value[..end]).is_err());
        }
        let mut trailing = value.to_vec();
        trailing.push(0);
        assert!(AseStatus::parse(&trailing).is_err());
    }
    for invalid in [
        &[7, 7][..],
        &[0, 0][..],
        &[7, 0, 0][..],
        &[7, 4, 0xf0, 0, 0][..],
        &[7, 4, 0, 0, 1, 0][..],
    ] {
        assert!(AseStatus::parse(invalid).is_err());
    }
}

#[test]
fn source_procedures_wait_for_response_and_state_in_either_order() {
    let mut client = AseClient::new(7, AseDirection::Source).unwrap();
    assert_eq!(
        client.begin(AseCommand::Enable(&[])),
        Err(ProfileError::NotSynchronized)
    );
    assert!(!client.observe_ase(QOS_STATE).unwrap());
    assert_eq!(client.begin(AseCommand::Enable(&[])).unwrap(), [3, 1, 7, 0]);
    assert_eq!(client.begin(AseCommand::Release), Err(ProfileError::Busy));
    assert!(!client.observe_response(&[3, 1, 7, 0, 0]).unwrap());
    assert!(client.is_pending());
    assert!(client.observe_ase(&[7, 3, 1, 2, 0]).unwrap());
    client.begin(AseCommand::ReceiverStartReady).unwrap();
    assert!(!client.observe_ase(&[7, 4, 1, 2, 0]).unwrap());
    assert!(client.observe_response(&[4, 1, 7, 0, 0]).unwrap());
    assert_eq!(client.state(), Some(4));
    client.begin(AseCommand::Disable).unwrap();
    assert!(!client.observe_response(&[5, 1, 7, 0, 0]).unwrap());
    assert!(client.observe_ase(&[7, 5, 1, 2, 0]).unwrap());
    client.begin(AseCommand::ReceiverStopReady).unwrap();
    assert!(!client.observe_ase(QOS_STATE).unwrap());
    assert!(client.observe_response(&[6, 1, 7, 0, 0]).unwrap());
    client.begin(AseCommand::Release).unwrap();
    assert!(!client.observe_response(&[8, 1, 7, 0, 0]).unwrap());
    assert!(!client.observe_ase(&[7, 6]).unwrap());
    assert!(client.observe_ase(&[7, 0]).unwrap());
    assert_eq!(client.state(), Some(0));
    assert!(!client.is_pending());
}

#[test]
fn sink_direction_rejection_wrong_notifications_and_disconnect_are_explicit() {
    let mut client = AseClient::new(7, AseDirection::Sink).unwrap();
    client.observe_ase(QOS_STATE).unwrap();
    assert_eq!(
        client.begin(AseCommand::ReceiverStartReady),
        Err(ProfileError::Transition)
    );
    client.begin(AseCommand::Enable(&[])).unwrap();
    assert_eq!(
        client.observe_response(&[2, 1, 7, 0, 0]),
        Err(ProfileError::UnexpectedResponse)
    );
    assert_eq!(
        client.observe_response(&[3, 1, 8, 0, 0]),
        Err(ProfileError::UnexpectedResponse)
    );
    assert_eq!(client.observe_ase(&[8, 0]), Err(ProfileError::AseId));
    assert_eq!(
        client.observe_response(&[3, 1, 7, 8, 2]),
        Err(ProfileError::Rejected { code: 8, reason: 2 })
    );
    assert_eq!(client.state(), Some(2));
    assert!(!client.is_pending());
    client.begin(AseCommand::Enable(&[])).unwrap();
    client.observe_ase(&[7, 4, 1, 2, 0]).unwrap();
    assert!(client.observe_response(&[3, 1, 7, 0, 0]).unwrap());
    client.begin(AseCommand::Disable).unwrap();
    client.observe_response(&[5, 1, 7, 0, 0]).unwrap();
    assert!(client.observe_ase(QOS_STATE).unwrap());
    client.begin(AseCommand::Release).unwrap();
    client.reset();
    assert_eq!(client.state(), None);
    assert!(!client.is_pending());
    assert_eq!(
        client.observe_response(&[8, 1, 7, 0, 0]),
        Err(ProfileError::UnexpectedResponse)
    );
    assert_eq!(
        client.begin(AseCommand::Release),
        Err(ProfileError::NotSynchronized)
    );
}
