//! Composition with the existing Trouble LE Audio stack; no replacement GATT.

use cyw43_iso_re::{
    bap::{AseCommand, ControlResponse, ProfileError},
    lc3::{CodecError, Decoder},
    le_audio::Lc3Config,
};
use trouble_audio::{
    DiscoveryError,
    ascs::{AscsClient, AseControlPointOperation},
    cis::RawLc3,
};
use trouble_host::prelude::*;

#[derive(Debug)]
pub enum ConfigureError<E> {
    Configuration(ProfileError),
    Discovery(DiscoveryError<E>),
    Host(BleHostError<E>),
    Wire,
    Rejected { code: u8, reason: u8 },
}

/// Discovers real ASCS and configures an ASE using Trouble's GATT client.
///
/// Caller first negotiates PACS, authenticates the connection, runs the GATT
/// client's task concurrently, and applies its own timeout. This returns after
/// the peer's control response; caller must still observe Codec Configured ASE
/// status before configuring QoS. No connection is opened by the host harness.
pub async fn configure_lc3<T: Controller, P: PacketPool, const SERVICES: usize>(
    client: &GattClient<'_, T, P, SERVICES>,
    ase_id: u8,
    config: Lc3Config,
) -> Result<AscsClient, ConfigureError<T::Error>> {
    let wire = AseCommand::ConfigureCodec {
        target_latency: 2,
        target_phy: 2,
        config,
    }
    .encode(ase_id)
    .map_err(ConfigureError::Configuration)?;
    let operation = AseControlPointOperation::from_gatt(&wire).map_err(|_| ConfigureError::Wire)?;
    let ascs = AscsClient::new(client)
        .await
        .map_err(ConfigureError::Discovery)?;
    let mut listener = client
        .subscribe(&ascs.ase_control_point, false)
        .await
        .map_err(ConfigureError::Host)?;
    client
        .write_characteristic(&ascs.ase_control_point, operation.as_gatt())
        .await
        .map_err(ConfigureError::Host)?;
    let (code, reason) = loop {
        let notification = listener.next().await;
        let response =
            ControlResponse::parse(notification.as_ref()).map_err(ConfigureError::Configuration)?;
        if response.opcode != 1 {
            continue;
        }
        // Upstream's response parser omits Number_of_ASEs = 0xff global errors.
        // Reuse its GATT client, with our complete response parser at the boundary.
        if let Some((_, code, reason)) = response
            .entries()
            .find(|(id, _, _)| response.global_error || *id == ase_id)
        {
            break (code, reason);
        }
    };
    drop(listener);
    if code == 0 {
        Ok(ascs)
    } else {
        Err(ConfigureError::Rejected { code, reason })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReceiveError {
    WrongAse,
    Discontinuity { expected: u16, received: u16 },
    Codec(CodecError),
}

/// Decodes one passthrough frame, emitting PCM for missing SDUs before live PCM.
///
/// One decoder/sequence state per mono ASE. `CisManager::new_passthrough()` sends
/// valid SDUs via `receive_lc3`; its lost SDUs manifest as sequence gaps. The first
/// received SDU establishes sequence baseline, so initial loss needs a separate
/// presentation timer. On reconnect or large discontinuity, reconstruct decoder
/// and clear `next_sequence`. Timing and audio-device writes remain caller-owned.
pub fn decode_received(
    decoder: &mut Decoder<'_, 1>,
    ase_id: u8,
    received: &RawLc3,
    next_sequence: &mut Option<u16>,
    pcm: &mut [i16],
    mut emit: impl FnMut(&[i16], bool),
) -> Result<(), ReceiveError> {
    if received.ase_id != ase_id {
        return Err(ReceiveError::WrongAse);
    }
    if received.frame.len() != decoder.layout().sdu_bytes()
        || pcm.len() != decoder.layout().pcm_samples()
    {
        return Err(ReceiveError::Codec(CodecError::SduLength));
    }
    let expected = next_sequence.unwrap_or(received.sequence_number);
    let missing = received.sequence_number.wrapping_sub(expected);
    // ponytail: conceal at most four SDUs; larger gaps require stream resync.
    if missing > 4 {
        return Err(ReceiveError::Discontinuity {
            expected,
            received: received.sequence_number,
        });
    }
    for _ in 0..missing {
        decoder.decode_sdu(None, pcm).map_err(ReceiveError::Codec)?;
        emit(pcm, true);
    }
    decoder
        .decode_sdu(Some(received.frame.as_slice()), pcm)
        .map_err(ReceiveError::Codec)?;
    emit(pcm, false);
    *next_sequence = Some(received.sequence_number.wrapping_add(1));
    Ok(())
}
