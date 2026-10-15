use cyw43_controller_proto::{
    isoal::{Unframed, UnframedSdu},
    ltv::{Lc3Config, UnicastQos},
};
use cyw43_iso_re::{
    iso::{Direction, PacketStatus, SduHeader},
    iso_credit::{CreditError, IsoCredits},
    iso_sdu::{SduFragments, SduReassembler},
};

#[test]
fn lc3_sized_sdu_crosses_credit_limited_hci_and_unframed_isoal() {
    let config = Lc3Config::parse(&[2, 1, 8, 2, 2, 1, 3, 4, 120, 0]).unwrap();
    UnicastQos {
        sdu_interval_us: 10_000,
        framing: 0,
        phy: 2,
        max_sdu: 120,
        max_transport_latency_ms: 20,
        presentation_delay_us: 40_000,
    }
    .validate_for(&config)
    .unwrap();

    // Synthetic codec-sized bytes exercise transport; no LC3 encoding is claimed.
    let payload = vec![0xa5; config.sdu_length().unwrap()];
    let header = SduHeader {
        timestamp: Some(10_000),
        sequence_number: u16::MAX,
        length: u16::try_from(payload.len()).unwrap(),
        status: PacketStatus::Valid,
    };
    let mut credits = IsoCredits::from_buffer_size_v2(&[0, 0xfb, 0, 4, 32, 0, 2]).unwrap();
    let fragments = SduFragments::new(
        0x123,
        header,
        &payload,
        usize::from(credits.packet_length()),
        Direction::HostToController,
    )
    .unwrap();
    let mut reassembler = SduReassembler::new(Direction::ControllerToHost, 1, 120).unwrap();
    let mut completed = None;
    let mut packet_count = 0;
    let mut backpressure_count = 0;
    for packet in fragments {
        if credits.available() == 0 {
            assert_eq!(credits.reserve(&packet), Err(CreditError::Exhausted));
            credits.complete(0x123, 2).unwrap();
            backpressure_count += 1;
        }
        credits.reserve(&packet).unwrap();
        let wire = packet.encode(Direction::HostToController).unwrap();
        assert!(wire.len() <= 36);
        let received = reassembler.push(&wire).unwrap();
        assert!(received.abandoned.is_none());
        if let Some(sdu) = received.completed {
            assert!(completed.replace(sdu).is_none());
        }
        packet_count += 1;
    }
    assert_eq!((packet_count, backpressure_count), (4, 1));
    credits.complete(0x123, 2).unwrap();
    assert_eq!(credits.available(), 2);
    let received = completed.unwrap();
    assert_eq!(received.handle, 0x123);
    assert_eq!(received.header, header);
    assert_eq!(received.payload, payload);

    let adaptation = Unframed::new(50, 4, 120, 10_000, 10_000).unwrap();
    let mut slots: Vec<_> = adaptation
        .segment(&received.payload)
        .unwrap()
        .map(Some)
        .collect();
    assert_eq!(
        slots
            .iter()
            .map(|slot| slot.unwrap().llid)
            .collect::<Vec<_>>(),
        [1, 1, 0, 1]
    );
    slots[3] = None; // Missing trailing padding does not lose audio.
    assert_eq!(
        adaptation.recombine(&slots).unwrap(),
        UnframedSdu::Valid(payload)
    );
    slots[1] = None;
    assert_eq!(adaptation.recombine(&slots).unwrap(), UnframedSdu::Lost);
}

#[cfg(feature = "lc3")]
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end audio/loss scenario keeps all boundary checks visible"
)]
fn encoded_audio_crosses_framed_isoal_hci_and_recovers_after_radio_loss() {
    use cyw43_controller_proto::isoal_framed::{
        FRAMED_LLID, Framed, FramedPdu, FramedReceiver, FramedReport, TimedSdu,
    };
    use cyw43_iso_re::lc3::{Complex, Decoder, Encoder};

    let config = Lc3Config::parse(&[2, 1, 3, 2, 2, 1, 3, 4, 40, 0]).unwrap();
    let (integers, scalars, complex) = Encoder::<1>::working_lengths(config).unwrap();
    let (mut integers, mut scalars, mut complex) = (
        vec![0; integers],
        vec![0.0; scalars],
        vec![Complex::default(); complex],
    );
    let mut encoder = Encoder::<1>::new(config, &mut integers, &mut scalars, &mut complex).unwrap();
    let (scalars, complex) = Decoder::<1>::working_lengths(config).unwrap();
    let (mut receive_scalars, mut receive_complex) =
        (vec![0.0; scalars], vec![Complex::default(); complex]);
    let (mut reference_scalars, mut reference_complex) =
        (vec![0.0; scalars], vec![Complex::default(); complex]);
    let mut decoder =
        Decoder::<1>::new(config, &mut receive_scalars, &mut receive_complex).unwrap();
    let mut reference =
        Decoder::<1>::new(config, &mut reference_scalars, &mut reference_complex).unwrap();
    let adaptation = Framed::new(20, 40, 10_000).unwrap();
    let mut receiver = FramedReceiver::new(adaptation, 30_000, 0, Some(31_000));
    let mut hci = SduReassembler::new(Direction::ControllerToHost, 1, 40).unwrap();
    let mut credits = IsoCredits::from_buffer_size_v2(&[0, 0, 0, 0, 24, 0, 1]).unwrap();
    for sequence in 0..3_u16 {
        let pcm: Vec<i16> = (0..160).map(|i| (i % 16 - 8) * 1000).collect();
        let mut compressed = [0; 40];
        encoder.encode_sdu(&pcm, &mut compressed).unwrap();
        // Host/controller boundary before ISOAL: real codec data, one ISO credit.
        let timestamp = 1_000 + u32::from(sequence) * 10_000;
        let header = SduHeader {
            timestamp: Some(timestamp),
            sequence_number: sequence,
            length: 40,
            status: PacketStatus::Valid,
        };
        let mut controller = SduReassembler::new(Direction::HostToController, 1, 40).unwrap();
        let mut controller_sdu = None;
        for packet in
            SduFragments::new(1, header, &compressed, 24, Direction::HostToController).unwrap()
        {
            credits.reserve(&packet).unwrap();
            assert_eq!(credits.reserve(&packet), Err(CreditError::Exhausted));
            controller_sdu = controller
                .push(&packet.encode(Direction::HostToController).unwrap())
                .unwrap()
                .completed;
            credits.complete(1, 1).unwrap();
        }
        let controller_sdu = controller_sdu.unwrap();
        let source = [TimedSdu {
            timestamp_us: timestamp,
            payload: &controller_sdu.payload,
        }];
        let mut fragments = adaptation.segment(&source).unwrap();
        let mut reports = Vec::new();
        for slot in 0..3 {
            let bytes = fragments.next_pdu(timestamp + 4_000).unwrap();
            let radio_pdu = if sequence == 1 && slot == 1 {
                None
            } else {
                Some(FramedPdu {
                    llid: FRAMED_LLID,
                    payload: &bytes,
                })
            };
            reports.extend(
                receiver
                    .receive(u64::from(sequence) * 3 + slot, timestamp + 4_000, radio_pdu)
                    .unwrap(),
            );
        }
        assert!(fragments.is_complete());
        assert_eq!(reports.len(), 1);
        let (header, payload) = match &reports[0] {
            FramedReport::Valid {
                sequence_number,
                timestamp_us,
                payload,
            } => (
                SduHeader {
                    timestamp: Some(*timestamp_us),
                    sequence_number: *sequence_number,
                    length: 40,
                    status: PacketStatus::Valid,
                },
                &payload[..],
            ),
            FramedReport::Lost {
                first_sequence_number,
                first_timestamp_us,
                count,
            } => {
                assert_eq!((*count, sequence), (1, 1));
                (
                    SduHeader {
                        timestamp: Some(*first_timestamp_us),
                        sequence_number: *first_sequence_number,
                        length: 0,
                        status: PacketStatus::Lost,
                    },
                    &[][..],
                )
            }
        };
        let mut completed = None;
        for packet in
            SduFragments::new(1, header, payload, 24, Direction::ControllerToHost).unwrap()
        {
            completed = hci
                .push(&packet.encode(Direction::ControllerToHost).unwrap())
                .unwrap()
                .completed;
        }
        let sdu = completed.unwrap();
        assert_eq!(sdu.header.sequence_number, sequence);
        assert_eq!(sdu.header.timestamp, Some(timestamp + 30_000));
        let codec_input =
            (sdu.header.status == PacketStatus::Valid).then_some(sdu.payload.as_slice());
        let expected = (sequence != 1).then_some(compressed.as_slice());
        let (mut actual_pcm, mut expected_pcm) = ([0; 160], [0; 160]);
        decoder.decode_sdu(codec_input, &mut actual_pcm).unwrap();
        reference.decode_sdu(expected, &mut expected_pcm).unwrap();
        assert_eq!(actual_pcm, expected_pcm);
    }
    assert_eq!(credits.available(), 1);
}
