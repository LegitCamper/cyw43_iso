//! GATT-independent PACS discovery and ASCS client wire primitives.
//!
//! Wire layout cross-checked against Zephyr v4.2.0 `ascs_internal.h` and
//! `pacs_internal.h`. Caller supplies complete characteristic values and writes
//! returned control-point bytes through its authenticated GATT connection.
//! Successful command encoding is not an ASE state transition: observe server
//! control-point responses and ASE notifications before advancing a stream.

use crate::le_audio::{AudioError, Lc3Config, UnicastQos};
use alloc::{vec, vec::Vec};
use core::fmt;

/// Bluetooth LC3 `Codec_ID`: coding format, company ID, vendor codec ID.
pub const LC3_CODEC_ID: [u8; 5] = [0x06, 0, 0, 0, 0];
/// Published Audio Capabilities Service UUID.
pub const PACS_UUID: u16 = 0x1850;
/// Audio Stream Control Service UUID.
pub const ASCS_UUID: u16 = 0x184e;
/// Sink PAC characteristic UUID.
pub const SINK_PAC_UUID: u16 = 0x2bc9;
/// Source PAC characteristic UUID.
pub const SOURCE_PAC_UUID: u16 = 0x2bcb;
/// Sink ASE characteristic UUID.
pub const SINK_ASE_UUID: u16 = 0x2bc4;
/// Source ASE characteristic UUID.
pub const SOURCE_ASE_UUID: u16 = 0x2bc5;
/// ASE Control Point characteristic UUID.
pub const ASE_CONTROL_POINT_UUID: u16 = 0x2bc6;

/// One borrowed PAC record. Unknown codec IDs remain available to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacRecord<'a> {
    pub codec_id: [u8; 5],
    pub capabilities: &'a [u8],
    pub metadata: &'a [u8],
}

/// Parses one complete Sink PAC or Source PAC characteristic value.
///
/// # Errors
/// Returns malformed lengths, invalid LTVs, or trailing bytes.
pub fn parse_pacs(mut bytes: &[u8]) -> Result<Vec<PacRecord<'_>>, ProfileError> {
    let count = take(&mut bytes, 1)?[0];
    let mut records = Vec::new();
    for _ in 0..count {
        let codec_id = take(&mut bytes, 5)?
            .try_into()
            .map_err(|_| ProfileError::Length)?;
        let capabilities = take_lv(&mut bytes)?;
        let metadata = take_lv(&mut bytes)?;
        validate_ltv(capabilities)?;
        validate_ltv(metadata)?;
        records.push(PacRecord {
            codec_id,
            capabilities,
            metadata,
        });
    }
    if !bytes.is_empty() {
        return Err(ProfileError::Length);
    }
    Ok(records)
}

/// Parsed LC3 capabilities; masks use Generic Audio Assigned Numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lc3Capabilities {
    pub frequencies: u16,
    pub durations: u8,
    pub channel_counts: u8,
    pub min_octets_per_frame: u16,
    pub max_octets_per_frame: u16,
    pub max_frames_per_sdu: u8,
}

impl Lc3Capabilities {
    /// Parses LC3 capability LTVs, with BAP defaults for channel/frame counts.
    /// Unknown types are ignored for forward compatibility.
    ///
    /// # Errors
    /// Returns malformed, duplicate, missing, or invalid known fields.
    pub fn parse(mut bytes: &[u8]) -> Result<Self, ProfileError> {
        let mut fields: [Option<&[u8]>; 5] = [None; 5];
        while !bytes.is_empty() {
            let item = take_lv(&mut bytes)?;
            let (&kind, value) = item.split_first().ok_or(ProfileError::Ltv)?;
            if (1..=5).contains(&kind) {
                let index = usize::from(kind - 1);
                if fields[index].is_some() || value.len() != [2, 1, 1, 4, 1][index] {
                    return Err(ProfileError::Capabilities);
                }
                fields[index] = Some(value);
            }
        }
        let frequencies = fields[0].ok_or(ProfileError::Capabilities)?;
        let octets = fields[3].ok_or(ProfileError::Capabilities)?;
        let result = Self {
            frequencies: u16::from_le_bytes([frequencies[0], frequencies[1]]),
            durations: fields[1].ok_or(ProfileError::Capabilities)?[0],
            channel_counts: fields[2].map_or(1, |v| v[0]),
            min_octets_per_frame: u16::from_le_bytes([octets[0], octets[1]]),
            max_octets_per_frame: u16::from_le_bytes([octets[2], octets[3]]),
            max_frames_per_sdu: fields[4].map_or(1, |v| v[0]),
        };
        result.validate()?;
        Ok(result)
    }

    #[allow(clippy::verbose_bit_mask)] // Assigned-number bit fields stay explicit.
    fn validate(self) -> Result<(), ProfileError> {
        if self.frequencies == 0
            || self.frequencies & 0xe000 != 0
            || self.durations & 0x03 == 0
            || self.durations & 0xcc != 0
            || self.durations & 0x30 == 0x30
            || (self.durations >> 4) & !self.durations != 0
            || self.channel_counts == 0
            || self.max_frames_per_sdu == 0
            || self.min_octets_per_frame == 0
            || self.min_octets_per_frame > self.max_octets_per_frame
        {
            return Err(ProfileError::Capabilities);
        }
        Ok(())
    }

    /// Checks every LC3 configuration field against peer capabilities.
    ///
    /// Audio Locations/context availability are separate PACS characteristics;
    /// caller must also check these before selecting a configuration.
    ///
    /// # Errors
    /// Returns invalid fields or unsupported configuration.
    pub fn supports(self, config: Lc3Config) -> Result<(), ProfileError> {
        self.validate()?;
        config.sdu_length().map_err(ProfileError::Audio)?;
        let channels = config.channel_count().map_err(ProfileError::Audio)?;
        if channels > 8
            || self.frequencies & (1 << (config.frequency as u8 - 1)) == 0
            || self.durations & (1 << config.duration as u8) == 0
            || self.channel_counts & (1 << (channels - 1)) == 0
            || !(self.min_octets_per_frame..=self.max_octets_per_frame)
                .contains(&config.octets_per_frame)
            || channels * u32::from(config.frame_blocks_per_sdu)
                > u32::from(self.max_frames_per_sdu)
        {
            return Err(ProfileError::UnsupportedConfiguration);
        }
        Ok(())
    }
}

/// ASCS client command for one ASE per write.
// ponytail: single ASE transactions; batch encoder can be added for synchronized groups.
#[derive(Debug, Clone, Copy)]
pub enum AseCommand<'a> {
    ConfigureCodec {
        target_latency: u8,
        target_phy: u8,
        config: Lc3Config,
    },
    ConfigureQos {
        cig_id: u8,
        cis_id: u8,
        qos: UnicastQos,
        retransmissions: u8,
        config: Lc3Config,
    },
    Enable(&'a [u8]),
    ReceiverStartReady,
    Disable,
    ReceiverStopReady,
    UpdateMetadata(&'a [u8]),
    Release,
}

impl AseCommand<'_> {
    /// Encodes one complete ASE Control Point write.
    ///
    /// Codec target PHY uses 1/2/3, while `QoS` PHY uses 1/2/4. Metadata is LTV.
    ///
    /// # Errors
    /// Returns invalid ASE ID, command fields, codec/QoS, or metadata.
    pub fn encode(self, ase_id: u8) -> Result<Vec<u8>, ProfileError> {
        if ase_id == 0 {
            return Err(ProfileError::AseId);
        }
        let opcode = match self {
            Self::ConfigureCodec { .. } => 1,
            Self::ConfigureQos { .. } => 2,
            Self::Enable(_) => 3,
            Self::ReceiverStartReady => 4,
            Self::Disable => 5,
            Self::ReceiverStopReady => 6,
            Self::UpdateMetadata(_) => 7,
            Self::Release => 8,
        };
        let mut bytes = vec![opcode, 1, ase_id];
        match self {
            Self::ConfigureCodec {
                target_latency,
                target_phy,
                config,
            } => {
                if !(1..=3).contains(&target_latency) || !(1..=3).contains(&target_phy) {
                    return Err(ProfileError::Command);
                }
                let codec = config.encode().map_err(ProfileError::Audio)?;
                bytes.extend_from_slice(&[target_latency, target_phy]);
                bytes.extend_from_slice(&LC3_CODEC_ID);
                bytes.push(u8::try_from(codec.len()).map_err(|_| ProfileError::Length)?);
                bytes.extend_from_slice(&codec);
            }
            Self::ConfigureQos {
                cig_id,
                cis_id,
                qos,
                retransmissions,
                config,
            } => {
                if cig_id > 0xef || cis_id > 0xef {
                    return Err(ProfileError::Command);
                }
                qos.validate_for(&config).map_err(ProfileError::Audio)?;
                bytes.extend_from_slice(&[cig_id, cis_id]);
                bytes.extend_from_slice(&qos.sdu_interval_us.to_le_bytes()[..3]);
                bytes.extend_from_slice(&[qos.framing, qos.phy]);
                bytes.extend_from_slice(&qos.max_sdu.to_le_bytes());
                bytes.push(retransmissions);
                bytes.extend_from_slice(&qos.max_transport_latency_ms.to_le_bytes());
                bytes.extend_from_slice(&qos.presentation_delay_us.to_le_bytes()[..3]);
            }
            Self::Enable(metadata) | Self::UpdateMetadata(metadata) => {
                validate_ltv(metadata)?;
                bytes.push(u8::try_from(metadata.len()).map_err(|_| ProfileError::Length)?);
                bytes.extend_from_slice(metadata);
            }
            Self::ReceiverStartReady | Self::Disable | Self::ReceiverStopReady | Self::Release => {}
        }
        Ok(bytes)
    }
}

/// Borrowed control-point response, preserving status/reason triples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlResponse<'a> {
    pub opcode: u8,
    /// True for the special `Number_of_ASEs = 0xff` response.
    pub global_error: bool,
    entries: &'a [u8],
}

impl<'a> ControlResponse<'a> {
    /// Parses one complete notification, including global opcode/length errors.
    ///
    /// # Errors
    /// Returns inconsistent lengths, reserved response codes, or invalid IDs.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ProfileError> {
        let header = bytes.get(..2).ok_or(ProfileError::Length)?;
        let global_error = header[1] == 0xff;
        let count = if global_error {
            1
        } else {
            usize::from(header[1])
        };
        if count == 0 || bytes.len() != 2 + 3 * count {
            return Err(ProfileError::Length);
        }
        for entry in bytes[2..].as_chunks::<3>().0 {
            if entry[1] > 0x0e
                || (global_error && !matches!(entry[1], 1 | 2))
                || (!global_error
                    && ((entry[0] == 0 && entry[1] != 3) || matches!(entry[1], 1 | 2)))
                || (entry[1] == 0 && entry[2] != 0)
            {
                return Err(ProfileError::Response);
            }
        }
        Ok(Self {
            opcode: header[0],
            global_error,
            entries: &bytes[2..],
        })
    }

    /// Iterates `(ASE_ID, Response_Code, Reason)` without allocating.
    pub fn entries(self) -> impl Iterator<Item = (u8, u8, u8)> + 'a {
        self.entries
            .as_chunks::<3>()
            .0
            .iter()
            .map(|entry| (entry[0], entry[1], entry[2]))
    }
}

/// Codec-configured ASE's server preferences (not negotiated transport values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecPreferences {
    pub framing: u8,
    pub phy: u8,
    pub retransmissions: u8,
    pub max_transport_latency_ms: u16,
    pub presentation_delay_min_us: u32,
    pub presentation_delay_max_us: u32,
    pub preferred_presentation_delay_min_us: u32,
    pub preferred_presentation_delay_max_us: u32,
}

/// Server-reported ASE state and its state-specific characteristic payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AseState<'a> {
    Idle,
    CodecConfigured {
        preferences: CodecPreferences,
        codec_id: [u8; 5],
        configuration: &'a [u8],
    },
    QosConfigured {
        cig_id: u8,
        cis_id: u8,
        qos: UnicastQos,
        retransmissions: u8,
    },
    Enabling {
        cig_id: u8,
        cis_id: u8,
        metadata: &'a [u8],
    },
    Streaming {
        cig_id: u8,
        cis_id: u8,
        metadata: &'a [u8],
    },
    Disabling {
        cig_id: u8,
        cis_id: u8,
        metadata: &'a [u8],
    },
    Releasing,
}

/// Complete Sink/Source ASE read or notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AseStatus<'a> {
    pub ase_id: u8,
    pub state: AseState<'a>,
}

impl<'a> AseStatus<'a> {
    /// Decodes exact state-specific wire layout, preserving codec/preferences.
    ///
    /// Codec support and transport/preference compatibility require separate
    /// negotiation checks; a parsed notification alone does not validate them.
    ///
    /// # Errors
    /// Returns unknown state, malformed lengths/LTVs, reserved CIG/CIS IDs, or
    /// zero ASE ID. Extra bytes are rejected for every state.
    pub fn parse(mut bytes: &'a [u8]) -> Result<Self, ProfileError> {
        let header = take(&mut bytes, 2)?;
        if header[0] == 0 {
            return Err(ProfileError::AseId);
        }
        let state = match header[1] {
            0 => AseState::Idle,
            1 => {
                let fields = take(&mut bytes, 22)?;
                let configuration = take_lv(&mut bytes)?;
                validate_ltv(configuration)?;
                AseState::CodecConfigured {
                    preferences: CodecPreferences {
                        framing: fields[0],
                        phy: fields[1],
                        retransmissions: fields[2],
                        max_transport_latency_ms: u16::from_le_bytes([fields[3], fields[4]]),
                        presentation_delay_min_us: u24(&fields[5..8]),
                        presentation_delay_max_us: u24(&fields[8..11]),
                        preferred_presentation_delay_min_us: u24(&fields[11..14]),
                        preferred_presentation_delay_max_us: u24(&fields[14..17]),
                    },
                    codec_id: fields[17..22]
                        .try_into()
                        .map_err(|_| ProfileError::Length)?,
                    configuration,
                }
            }
            2 => {
                let fields = take(&mut bytes, 15)?;
                check_stream_ids(fields[0], fields[1])?;
                AseState::QosConfigured {
                    cig_id: fields[0],
                    cis_id: fields[1],
                    retransmissions: fields[9],
                    qos: UnicastQos {
                        sdu_interval_us: u24(&fields[2..5]),
                        framing: fields[5],
                        phy: fields[6],
                        max_sdu: u16::from_le_bytes([fields[7], fields[8]]),
                        max_transport_latency_ms: u16::from_le_bytes([fields[10], fields[11]]),
                        presentation_delay_us: u24(&fields[12..15]),
                    },
                }
            }
            3..=5 => {
                let ids = take(&mut bytes, 2)?;
                check_stream_ids(ids[0], ids[1])?;
                let metadata = take_lv(&mut bytes)?;
                validate_ltv(metadata)?;
                match header[1] {
                    3 => AseState::Enabling {
                        cig_id: ids[0],
                        cis_id: ids[1],
                        metadata,
                    },
                    4 => AseState::Streaming {
                        cig_id: ids[0],
                        cis_id: ids[1],
                        metadata,
                    },
                    _ => AseState::Disabling {
                        cig_id: ids[0],
                        cis_id: ids[1],
                        metadata,
                    },
                }
            }
            6 => AseState::Releasing,
            _ => return Err(ProfileError::State),
        };
        if !bytes.is_empty() {
            return Err(ProfileError::Length);
        }
        Ok(Self {
            ase_id: header[0],
            state,
        })
    }
}

fn check_stream_ids(group: u8, stream: u8) -> Result<(), ProfileError> {
    if group > 0xef || stream > 0xef {
        return Err(ProfileError::Command);
    }
    Ok(())
}

fn u24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

/// ASE direction from the server's perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AseDirection {
    Sink,
    Source,
}

/// GATT-independent single-ASE procedure tracker.
///
/// Feed notifications in ATT delivery order. Read/notify the ASE initially,
/// call `begin`, send its bytes as a GATT Write Request, then feed control-point
/// and ASE notifications. A procedure completes only after both a successful
/// control response and a target-state notification, in either order.
///
/// This tracks server state, not CIS establishment or audio readiness. Caller
/// owns ATT discovery/security, writes, subscription, timeouts, and reconnects.
#[derive(Debug)]
pub struct AseClient {
    ase_id: u8,
    direction: AseDirection,
    state: Option<u8>,
    pending: Option<PendingProcedure>,
}

#[derive(Debug)]
struct PendingProcedure {
    opcode: u8,
    target_states: u8,
    response_seen: bool,
    state_seen: bool,
    release_started: bool,
}

impl AseClient {
    /// Creates an unsynchronized ASE; an initial read/notification is required.
    ///
    /// # Errors
    /// Returns zero ASE ID.
    pub fn new(ase_id: u8, direction: AseDirection) -> Result<Self, ProfileError> {
        if ase_id == 0 {
            return Err(ProfileError::AseId);
        }
        Ok(Self {
            ase_id,
            direction,
            state: None,
            pending: None,
        })
    }

    /// Last observed ASCS state assigned number (0..=6), or unknown after reset.
    #[must_use]
    pub const fn state(&self) -> Option<u8> {
        self.state
    }

    /// Whether a control response or target-state notification is still pending.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Resets on disconnect, ATT write failure, or procedure timeout.
    /// Drain old notifications before resynchronizing from a fresh ASE read.
    pub fn reset(&mut self) {
        self.state = None;
        self.pending = None;
    }

    /// Checks server state/direction and begins one control-point write.
    ///
    /// `ReceiverStartReady` requires caller to have established the receiving
    /// CIS/data path; `ReceiverStopReady` requires it to have stopped reception.
    ///
    /// # Errors
    /// Returns overlapping procedure, missing initial state, illegal transition,
    /// or invalid command fields. Failure leaves pending state unchanged.
    pub fn begin(&mut self, command: AseCommand<'_>) -> Result<Vec<u8>, ProfileError> {
        if self.pending.is_some() {
            return Err(ProfileError::Busy);
        }
        let state = self.state.ok_or(ProfileError::NotSynchronized)?;
        let (allowed, target_states) = match command {
            AseCommand::ConfigureCodec { .. } => (state <= 2, 1 << 1),
            AseCommand::ConfigureQos { .. } => (matches!(state, 1 | 2), 1 << 2),
            AseCommand::Enable(_) => (
                state == 2,
                if self.direction == AseDirection::Source {
                    1 << 3
                } else {
                    (1 << 3) | (1 << 4)
                },
            ),
            AseCommand::ReceiverStartReady => {
                (self.direction == AseDirection::Source && state == 3, 1 << 4)
            }
            AseCommand::Disable => (
                matches!(state, 3 | 4),
                if self.direction == AseDirection::Source {
                    1 << 5
                } else {
                    1 << 2
                },
            ),
            AseCommand::ReceiverStopReady => {
                (self.direction == AseDirection::Source && state == 5, 1 << 2)
            }
            AseCommand::UpdateMetadata(_) => (matches!(state, 3 | 4), (1 << 3) | (1 << 4)),
            AseCommand::Release => ((1..=5).contains(&state), (1 << 0) | (1 << 1)),
        };
        if !allowed {
            return Err(ProfileError::Transition);
        }
        let bytes = command.encode(self.ase_id)?;
        self.pending = Some(PendingProcedure {
            opcode: bytes[0],
            target_states,
            response_seen: false,
            state_seen: false,
            release_started: false,
        });
        Ok(bytes)
    }

    /// Accepts a complete ASE characteristic value. Returns true on completion.
    ///
    /// Unsolicited state changes update the observed state. Intermediate states
    /// (e.g. Releasing) do not complete a pending procedure.
    ///
    /// # Errors
    /// Returns malformed notification or a different ASE ID.
    pub fn observe_ase(&mut self, bytes: &[u8]) -> Result<bool, ProfileError> {
        let status = AseStatus::parse(bytes)?;
        if status.ase_id != self.ase_id {
            return Err(ProfileError::AseId);
        }
        let state = match status.state {
            AseState::Idle => 0,
            AseState::CodecConfigured { .. } => 1,
            AseState::QosConfigured { .. } => 2,
            AseState::Enabling { .. } => 3,
            AseState::Streaming { .. } => 4,
            AseState::Disabling { .. } => 5,
            AseState::Releasing => 6,
        };
        self.state = Some(state);
        if let Some(pending) = self.pending.as_mut() {
            pending.release_started |= pending.opcode == 8 && state == 6;
            pending.state_seen |= pending.target_states & (1 << state) != 0
                && (pending.opcode != 8 || pending.release_started);
        }
        Ok(self.complete_if_ready())
    }

    /// Accepts one control-point response. Returns true on completion.
    ///
    /// Rejection ends the procedure without inventing a new ASE state. Unexpected
    /// opcode/ASE responses leave the pending procedure unchanged.
    ///
    /// # Errors
    /// Returns malformed/mismatched response or peer rejection with status/reason.
    pub fn observe_response(&mut self, bytes: &[u8]) -> Result<bool, ProfileError> {
        let response = ControlResponse::parse(bytes)?;
        let pending = self
            .pending
            .as_mut()
            .ok_or(ProfileError::UnexpectedResponse)?;
        let mut entries = response.entries();
        let (id, code, reason) = entries.next().ok_or(ProfileError::Response)?;
        if response.opcode != pending.opcode
            || pending.response_seen
            || entries.next().is_some()
            || (!response.global_error && id != self.ase_id)
        {
            return Err(ProfileError::UnexpectedResponse);
        }
        if code != 0 {
            self.pending = None;
            return Err(ProfileError::Rejected { code, reason });
        }
        pending.response_seen = true;
        Ok(self.complete_if_ready())
    }

    fn complete_if_ready(&mut self) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.response_seen && pending.state_seen)
        {
            self.pending = None;
            true
        } else {
            false
        }
    }
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], ProfileError> {
    let value = bytes.get(..length).ok_or(ProfileError::Length)?;
    *bytes = &bytes[length..];
    Ok(value)
}

fn take_lv<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8], ProfileError> {
    let length = usize::from(take(bytes, 1)?[0]);
    take(bytes, length)
}

fn validate_ltv(mut bytes: &[u8]) -> Result<(), ProfileError> {
    while !bytes.is_empty() {
        if take_lv(&mut bytes)?.is_empty() {
            return Err(ProfileError::Ltv);
        }
    }
    Ok(())
}

/// Profile wire or negotiation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileError {
    Length,
    Ltv,
    Capabilities,
    UnsupportedConfiguration,
    AseId,
    Command,
    Response,
    State,
    Busy,
    NotSynchronized,
    Transition,
    UnexpectedResponse,
    Rejected { code: u8, reason: u8 },
    Audio(AudioError),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BAP profile error: {self:?}")
    }
}

impl core::error::Error for ProfileError {}
