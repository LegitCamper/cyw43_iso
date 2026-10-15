//! HCI ISO control commands and asynchronous establishment tracking.
//!
//! Bluetooth Core 5.4, Vol 4, Part E, sections 7.8.97--110, 7.8.115 and
//! 7.7.65.25--29 define the wire formats and bounds used here:
//! <https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html>
//!
//! Commands exclude H4's command indicator. Callers own HCI command credits,
//! event masks, ACL/advertising prerequisites and controller capability checks.
//! Command Status success means accepted, never established. Nothing here adds
//! ISO support to firmware that rejects these commands.

use alloc::{vec, vec::Vec};
use core::fmt;

use crate::iso::MAX_HANDLE;

/// Invalid host parameters, controller framing, or procedure ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlError {
    /// Field, range, duplicate ID, or cross-field constraint is invalid.
    Parameter,
    /// HCI command parameters exceed their one-octet length field.
    CommandLength,
    /// Controller event has invalid length, identity, or negotiated values.
    Event,
    /// Event/command does not belong to the active procedure or arrives twice.
    Procedure,
    /// Controller returned this nonzero HCI status.
    Controller(u8),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parameter => f.write_str("invalid ISO control parameter"),
            Self::CommandLength => f.write_str("ISO HCI command exceeds 255 parameter octets"),
            Self::Event => f.write_str("malformed ISO controller event"),
            Self::Procedure => f.write_str("unexpected ISO procedure event"),
            Self::Controller(status) => {
                write!(f, "controller rejected ISO command: 0x{status:02x}")
            }
        }
    }
}

impl core::error::Error for ControlError {}

/// One CIS configuration; directional arrays are central-to-peripheral first.
#[derive(Debug, Clone, Copy)]
pub struct CisParameters {
    /// CIS ID in 0..=0xef, unique within this command.
    pub id: u8,
    /// Maximum SDU bytes in each direction (0 disables that direction).
    pub max_sdu: [u16; 2],
    /// Allowed PHY bit masks (1=1M, 2=2M, 4=Coded); each must be nonzero.
    pub phy: [u8; 2],
    /// Recommended retransmissions, each 0..=255 for CIS.
    pub retransmissions: [u8; 2],
}

/// CIG configuration; directional arrays are central-to-peripheral first.
#[derive(Debug, Clone, Copy)]
pub struct CigParameters<'a> {
    /// CIG ID in 0..=0xef.
    pub id: u8,
    /// SDU intervals in microseconds, each 255..=0xfffff.
    pub sdu_interval_us: [u32; 2],
    /// Worst peripheral sleep clock accuracy encoding, 0..=7.
    pub worst_case_sca: u8,
    /// Prefer interleaved subevents instead of sequential packing.
    pub interleaved: bool,
    /// Require framed PDUs; false permits controller-selected framing.
    pub framed: bool,
    /// Maximum transport latencies in milliseconds, each 5..=4000.
    pub max_transport_latency_ms: [u16; 2],
    /// CISes to add/modify. One command fits at most 26; later commands may add
    /// the remaining CISes up to the CIG limit of 31.
    pub streams: &'a [CisParameters],
}

/// CIS configuration handle and its already-established ACL connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CisConnection {
    /// Handle returned by Set CIG Parameters.
    pub cis_handle: u16,
    /// Associated ACL handle, with the local device in the central role.
    pub acl_handle: u16,
}

/// Broadcast-source configuration for an existing periodic advertising train.
#[derive(Clone, Copy)]
pub struct BigParameters {
    /// Host-assigned BIG ID in 0..=0xef.
    pub handle: u8,
    /// Periodic advertising set handle in 0..=0xef.
    pub advertising_handle: u8,
    /// Number of BISes, 1..=31.
    pub bis_count: u8,
    /// SDU interval in microseconds, 255..=0xfffff.
    pub sdu_interval_us: u32,
    /// Maximum SDU length, 1..=4095 bytes.
    pub max_sdu: u16,
    /// Maximum transport latency, 5..=4000 milliseconds.
    pub max_transport_latency_ms: u16,
    /// Recommended retransmissions, 0..=30 for BIS.
    pub retransmissions: u8,
    /// Allowed PHY bit mask: bit 0=1M, bit 1=2M, bit 2=Coded.
    pub phy: u8,
    /// Prefer interleaved subevents.
    pub interleaved: bool,
    /// Require framed PDUs.
    pub framed: bool,
    /// Encryption code, or `None` for unencrypted transmission.
    pub broadcast_code: Option<[u8; 16]>,
}

/// Broadcast-receiver configuration for an already-synchronized periodic train.
#[derive(Clone, Copy)]
pub struct BigSyncParameters<'a> {
    /// Host-assigned BIG ID in 0..=0xef.
    pub handle: u8,
    /// Periodic advertising synchronization handle in 0..=0xeff.
    pub sync_handle: u16,
    /// Broadcast code for encrypted reception, or `None` for unencrypted.
    pub broadcast_code: Option<[u8; 16]>,
    /// Maximum subevents per BIS (0 means any), 0..=31.
    pub max_subevents: u8,
    /// Synchronization timeout in 10 ms units, 10..=0x4000.
    pub timeout: u16,
    /// Ascending unique BIS indices, each 1..=31; at least one required.
    pub bis_indices: &'a [u8],
}

/// Data path direction, from the controller's perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DataPathDirection {
    /// Host to controller.
    Input = 0,
    /// Controller to host.
    Output = 1,
}

/// Validated command with opcode and parameter bytes ready for an HCI transport.
/// Debug output deliberately excludes parameters, which can include broadcast keys.
#[derive(Clone)]
pub struct ControlCommand {
    opcode: u16,
    parameters: Vec<u8>,
}

impl fmt::Debug for ControlCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlCommand")
            .field("opcode", &self.opcode)
            .field("parameter_length", &self.parameters.len())
            .finish()
    }
}

#[cfg(feature = "embassy")]
impl bt_hci::HostToControllerPacket for ControlCommand {
    const KIND: bt_hci::PacketKind = bt_hci::PacketKind::Cmd;
}

#[cfg(feature = "embassy")]
impl bt_hci::WriteHci for ControlCommand {
    fn size(&self) -> usize {
        3 + self.parameters.len()
    }

    fn write_hci<W: embedded_io::Write>(&self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(&self.opcode.to_le_bytes())?;
        writer.write_all(&[self.parameters.len().to_le_bytes()[0]])?;
        writer.write_all(&self.parameters)
    }

    async fn write_hci_async<W: embedded_io_async::Write>(
        &self,
        mut writer: W,
    ) -> Result<(), W::Error> {
        writer.write_all(&self.opcode.to_le_bytes()).await?;
        writer
            .write_all(&[self.parameters.len().to_le_bytes()[0]])
            .await?;
        writer.write_all(&self.parameters).await
    }
}

fn require(condition: bool) -> Result<(), ControlError> {
    if condition {
        Ok(())
    } else {
        Err(ControlError::Parameter)
    }
}

fn put24(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes()[..3]);
}

fn interval(value: u32) -> bool {
    (255..=0x000f_ffff).contains(&value)
}

impl ControlCommand {
    fn new(opcode: u16, parameters: Vec<u8>) -> Result<Self, ControlError> {
        if parameters.len() > 255 {
            return Err(ControlError::CommandLength);
        }
        Ok(Self { opcode, parameters })
    }

    /// HCI opcode, including OGF.
    #[must_use]
    pub fn opcode(&self) -> u16 {
        self.opcode
    }

    /// Validated parameters without opcode, length, or H4 indicator.
    #[must_use]
    pub fn parameters(&self) -> &[u8] {
        &self.parameters
    }

    /// Encodes opcode, one-octet parameter length, and parameters (no H4 byte).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = self.opcode.to_le_bytes().to_vec();
        bytes.push(self.parameters.len().to_le_bytes()[0]);
        bytes.extend_from_slice(&self.parameters);
        bytes
    }

    /// Configures CIS resources; returned handles do not mean CISes exist yet.
    ///
    /// # Errors
    /// Rejects field bounds, duplicate IDs, inactive CISes, or overlong commands.
    pub fn set_cig(parameters: &CigParameters<'_>) -> Result<Self, ControlError> {
        require(
            parameters.id <= 0xef
                && parameters.worst_case_sca <= 7
                && parameters.sdu_interval_us.into_iter().all(interval)
                && parameters
                    .max_transport_latency_ms
                    .into_iter()
                    .all(|value| (5..=4000).contains(&value))
                && parameters.streams.len() <= 31,
        )?;
        let mut bytes = vec![parameters.id];
        for value in parameters.sdu_interval_us {
            put24(&mut bytes, value);
        }
        bytes.extend_from_slice(&[
            parameters.worst_case_sca,
            u8::from(parameters.interleaved),
            u8::from(parameters.framed),
        ]);
        for value in parameters.max_transport_latency_ms {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.push(parameters.streams.len().to_le_bytes()[0]);
        for (index, stream) in parameters.streams.iter().enumerate() {
            require(
                stream.id <= 0xef
                    && !parameters.streams[..index]
                        .iter()
                        .any(|prior| prior.id == stream.id)
                    && stream.max_sdu.into_iter().all(|size| size <= 4095)
                    && stream.max_sdu != [0, 0]
                    && stream.phy.into_iter().all(|phy| (1..=7).contains(&phy)),
            )?;
            bytes.push(stream.id);
            for value in stream.max_sdu {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&stream.phy);
            bytes.extend_from_slice(&stream.retransmissions);
        }
        Self::new(0x2062, bytes)
    }

    /// Starts central-side CIS creation on existing ACL links.
    ///
    /// # Errors
    /// Requires 1..=31 unique CIS handles and legal, distinct CIS/ACL handles.
    pub fn create_cis(connections: &[CisConnection]) -> Result<Self, ControlError> {
        require((1..=31).contains(&connections.len()))?;
        let mut bytes = vec![connections.len().to_le_bytes()[0]];
        for (index, connection) in connections.iter().enumerate() {
            require(
                connection.cis_handle <= MAX_HANDLE
                    && connection.acl_handle <= MAX_HANDLE
                    && connection.cis_handle != connection.acl_handle
                    && !connections[..index]
                        .iter()
                        .any(|prior| prior.cis_handle == connection.cis_handle)
                    && !connections
                        .iter()
                        .any(|other| other.acl_handle == connection.cis_handle),
            )?;
            bytes.extend_from_slice(&connection.cis_handle.to_le_bytes());
            bytes.extend_from_slice(&connection.acl_handle.to_le_bytes());
        }
        Self::new(0x2064, bytes)
    }

    /// Removes an inactive CIG; controller completion must confirm success.
    ///
    /// # Errors
    /// Rejects IDs above 0xef.
    pub fn remove_cig(id: u8) -> Result<Self, ControlError> {
        require(id <= 0xef)?;
        Self::new(0x2065, vec![id])
    }

    /// Accepts a peripheral-side CIS request previously reported by the controller.
    ///
    /// # Errors
    /// Rejects handles above 0xeff.
    pub fn accept_cis(handle: u16) -> Result<Self, ControlError> {
        require(handle <= MAX_HANDLE)?;
        Self::new(0x2066, handle.to_le_bytes().to_vec())
    }

    /// Rejects a pending CIS request with a nonzero controller error reason.
    ///
    /// # Errors
    /// Rejects invalid handles or a zero reason.
    pub fn reject_cis(handle: u16, reason: u8) -> Result<Self, ControlError> {
        require(handle <= MAX_HANDLE && reason != 0)?;
        let mut bytes = handle.to_le_bytes().to_vec();
        bytes.push(reason);
        Self::new(0x2067, bytes)
    }

    /// Starts BIG creation on an existing periodic advertising train.
    ///
    /// # Errors
    /// Rejects values outside Core bounds.
    pub fn create_big(parameters: &BigParameters) -> Result<Self, ControlError> {
        require(
            parameters.handle <= 0xef
                && parameters.advertising_handle <= 0xef
                && (1..=31).contains(&parameters.bis_count)
                && interval(parameters.sdu_interval_us)
                && (1..=4095).contains(&parameters.max_sdu)
                && (5..=4000).contains(&parameters.max_transport_latency_ms)
                && parameters.retransmissions <= 30
                && (1..=7).contains(&parameters.phy),
        )?;
        let mut bytes = vec![
            parameters.handle,
            parameters.advertising_handle,
            parameters.bis_count,
        ];
        put24(&mut bytes, parameters.sdu_interval_us);
        bytes.extend_from_slice(&parameters.max_sdu.to_le_bytes());
        bytes.extend_from_slice(&parameters.max_transport_latency_ms.to_le_bytes());
        bytes.extend_from_slice(&[
            parameters.retransmissions,
            parameters.phy,
            u8::from(parameters.interleaved),
            u8::from(parameters.framed),
            u8::from(parameters.broadcast_code.is_some()),
        ]);
        bytes.extend_from_slice(&parameters.broadcast_code.unwrap_or([0; 16]));
        Self::new(0x2068, bytes)
    }

    /// Stops a broadcast BIG asynchronously; wait for Terminate BIG Complete.
    ///
    /// # Errors
    /// Rejects IDs above 0xef or a zero reason.
    pub fn terminate_big(handle: u8, reason: u8) -> Result<Self, ControlError> {
        require(handle <= 0xef && reason != 0)?;
        Self::new(0x206a, vec![handle, reason])
    }

    /// Starts synchronization to selected BISes from a periodic advertising train.
    ///
    /// # Errors
    /// Rejects invalid bounds, empty lists, duplicate or unsorted BIS indices.
    pub fn sync_big(parameters: &BigSyncParameters<'_>) -> Result<Self, ControlError> {
        require(
            parameters.handle <= 0xef
                && parameters.sync_handle <= MAX_HANDLE
                && parameters.max_subevents <= 31
                && (10..=0x4000).contains(&parameters.timeout)
                && (1..=31).contains(&parameters.bis_indices.len())
                && parameters
                    .bis_indices
                    .iter()
                    .all(|index| (1..=31).contains(index))
                && parameters
                    .bis_indices
                    .windows(2)
                    .all(|pair| pair[0] < pair[1]),
        )?;
        let mut bytes = vec![parameters.handle];
        bytes.extend_from_slice(&parameters.sync_handle.to_le_bytes());
        bytes.push(u8::from(parameters.broadcast_code.is_some()));
        bytes.extend_from_slice(&parameters.broadcast_code.unwrap_or([0; 16]));
        bytes.push(parameters.max_subevents);
        bytes.extend_from_slice(&parameters.timeout.to_le_bytes());
        bytes.push(parameters.bis_indices.len().to_le_bytes()[0]);
        bytes.extend_from_slice(parameters.bis_indices);
        Self::new(0x206b, bytes)
    }

    /// Stops BIG synchronization; wait for Command Complete before retiring handles.
    ///
    /// # Errors
    /// Rejects IDs above 0xef.
    pub fn terminate_big_sync(handle: u8) -> Result<Self, ControlError> {
        require(handle <= 0xef)?;
        Self::new(0x206c, vec![handle])
    }

    /// Sets an HCI data path with transparent controller coding for host LC3 bytes.
    ///
    /// # Errors
    /// Rejects invalid handles or controller delays exceeding four seconds.
    pub fn setup_data_path(
        handle: u16,
        direction: DataPathDirection,
        controller_delay_us: u32,
    ) -> Result<Self, ControlError> {
        require(handle <= MAX_HANDLE && controller_delay_us <= 4_000_000)?;
        let mut bytes = handle.to_le_bytes().to_vec();
        // ponytail: host codec + HCI only; add vendor paths when hardware defines one.
        bytes.extend_from_slice(&[direction as u8, 0, 3, 0, 0, 0, 0]);
        put24(&mut bytes, controller_delay_us);
        bytes.push(0);
        Self::new(0x206e, bytes)
    }

    /// Removes input (bit 0), output (bit 1), or both data paths.
    ///
    /// # Errors
    /// Requires a valid handle and direction mask in 1..=3. This mask differs
    /// from Setup ISO Data Path's 0/1 direction enumeration.
    pub fn remove_data_path(handle: u16, directions: u8) -> Result<Self, ControlError> {
        require(handle <= MAX_HANDLE && (1..=3).contains(&directions))?;
        let mut bytes = handle.to_le_bytes().to_vec();
        bytes.push(directions);
        Self::new(0x206f, bytes)
    }

    /// Sets Connected Isochronous Stream (Host Support), feature bit 32.
    /// Issue before creating any ACL connection; controller support is required.
    #[must_use]
    pub fn set_iso_host_support(enabled: bool) -> Self {
        Self {
            opcode: 0x2074,
            parameters: vec![32, u8::from(enabled)],
        }
    }
}

/// Successful Set CIG Parameters return values; still only configurations.
#[derive(Debug, PartialEq, Eq)]
pub struct CigConfigured {
    /// CIG ID echoed by the controller.
    pub id: u8,
    /// CIS handles in command CIS-ID order.
    pub handles: Vec<u16>,
}

impl CigConfigured {
    /// Parses Command Complete return parameters, starting with Status.
    ///
    /// # Errors
    /// Preserves nonzero controller status; rejects malformed success data.
    pub fn parse(bytes: &[u8]) -> Result<Self, ControlError> {
        let status = *bytes.first().ok_or(ControlError::Event)?;
        if status != 0 {
            return Err(ControlError::Controller(status));
        }
        if bytes.len() < 3 || bytes[1] > 0xef || bytes[2] > 31 {
            return Err(ControlError::Event);
        }
        Ok(Self {
            id: bytes[1],
            handles: parse_handles(&bytes[3..], bytes[2])?,
        })
    }
}

fn parse_handles(bytes: &[u8], count: u8) -> Result<Vec<u16>, ControlError> {
    if bytes.len() != usize::from(count) * 2 {
        return Err(ControlError::Event);
    }
    let mut handles = Vec::with_capacity(usize::from(count));
    for pair in bytes.as_chunks::<2>().0 {
        let handle = u16::from_le_bytes(*pair);
        if handle > MAX_HANDLE || handles.contains(&handle) {
            return Err(ControlError::Event);
        }
        handles.push(handle);
    }
    Ok(handles)
}

fn read24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}
fn read16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

/// Negotiated CIS timing, available only after successful establishment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CisTiming {
    /// Group synchronization delay, microseconds.
    pub cig_sync_delay_us: u32,
    /// Stream synchronization delay, microseconds.
    pub cis_sync_delay_us: u32,
    /// Directional transport latency, central-to-peripheral first.
    pub transport_latency_us: [u32; 2],
    /// Selected PHY enumeration (1=1M, 2=2M, 3=Coded), not command bit mask.
    pub phy: [u8; 2],
    /// Maximum subevents per CIS event.
    pub subevents: u8,
    /// Directional burst numbers; 0 means inactive direction.
    pub burst_number: [u8; 2],
    /// Directional flush timeouts in ISO intervals.
    pub flush_timeout: [u8; 2],
    /// Directional maximum radio PDU payload lengths.
    pub max_pdu: [u16; 2],
    /// ISO interval in 1.25 ms units.
    pub iso_interval: u16,
}

/// Negotiated BIG timing and handles after broadcast creation or synchronization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigTiming {
    /// Present for broadcast creation; absent from BIG Sync Established.
    pub sync_delay_us: Option<u32>,
    /// Actual transport latency, microseconds.
    pub transport_latency_us: u32,
    /// Selected PHY (1/2/3), present only for broadcast creation.
    pub phy: Option<u8>,
    /// Number of BIS subevents per event.
    pub subevents: u8,
    /// Number of new payloads per BIS event.
    pub burst_number: u8,
    /// Pre-transmission offset in ISO intervals.
    pub pretransmission_offset: u8,
    /// Immediate repetition count.
    pub repetition_count: u8,
    /// Maximum radio PDU payload length.
    pub max_pdu: u16,
    /// ISO interval in 1.25 ms units.
    pub iso_interval: u16,
    /// BIS handles in requested order.
    pub handles: Vec<u16>,
}

/// Establishment target, disambiguating CIS handles from host-assigned BIG IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoTarget {
    /// One connected stream.
    Cis(u16),
    /// Broadcast-source group.
    Big(u8),
    /// Synchronized broadcast-receiver group.
    BigSync(u8),
}

/// Negotiated successful establishment details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstablishedParameters {
    /// Connected stream timing.
    Cis(CisTiming),
    /// Broadcast group timing and handles.
    Big(BigTiming),
}

/// Parsed asynchronous event, preserving controller failures without bogus timing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstablishedEvent {
    /// Requested CIS or BIG.
    pub target: IsoTarget,
    /// Exact HCI status, zero only on success.
    pub status: u8,
    /// Absent on failure, whose timing fields must be ignored.
    pub parameters: Option<EstablishedParameters>,
}

impl EstablishedEvent {
    /// Parses LE Meta event parameters including its subevent byte.
    ///
    /// # Errors
    /// Rejects truncated, oversized or invalid successful events. Failed events
    /// retain controller status and ignore undefined timing fields.
    pub fn parse(bytes: &[u8]) -> Result<Self, ControlError> {
        if bytes.len() < 3 {
            return Err(ControlError::Event);
        }
        match bytes[0] {
            0x19 => Self::cis(bytes),
            0x1b | 0x1d => Self::big(bytes),
            _ => Err(ControlError::Event),
        }
    }

    fn cis(bytes: &[u8]) -> Result<Self, ControlError> {
        if bytes.len() != 29 {
            return Err(ControlError::Event);
        }
        let handle = read16(&bytes[2..]);
        if handle > MAX_HANDLE {
            return Err(ControlError::Event);
        }
        let mut event = Self {
            target: IsoTarget::Cis(handle),
            status: bytes[1],
            parameters: None,
        };
        if event.status != 0 {
            return Ok(event);
        }
        let timing = CisTiming {
            cig_sync_delay_us: read24(&bytes[4..]),
            cis_sync_delay_us: read24(&bytes[7..]),
            transport_latency_us: [read24(&bytes[10..]), read24(&bytes[13..])],
            phy: [bytes[16], bytes[17]],
            subevents: bytes[18],
            burst_number: [bytes[19], bytes[20]],
            flush_timeout: [bytes[21], bytes[22]],
            max_pdu: [read16(&bytes[23..]), read16(&bytes[25..])],
            iso_interval: read16(&bytes[27..]),
        };
        if ![timing.cig_sync_delay_us, timing.cis_sync_delay_us]
            .into_iter()
            .all(delay_valid)
            || timing.cis_sync_delay_us > timing.cig_sync_delay_us
            || !timing.transport_latency_us.into_iter().all(delay_valid)
            || !timing.phy.into_iter().all(|phy| (1..=3).contains(&phy))
            || !(1..=31).contains(&timing.subevents)
            || timing.burst_number == [0, 0]
            || !timing.burst_number.into_iter().all(|burst| burst <= 15)
            || timing.flush_timeout.contains(&0)
            || timing.max_pdu.into_iter().any(|size| size > 251)
            || !(4..=3200).contains(&timing.iso_interval)
            || (0..2).any(|i| timing.burst_number[i] > 0 && timing.max_pdu[i] == 0)
        {
            return Err(ControlError::Event);
        }
        event.parameters = Some(EstablishedParameters::Cis(timing));
        Ok(event)
    }

    fn big(bytes: &[u8]) -> Result<Self, ControlError> {
        let broadcaster = bytes[0] == 0x1b;
        let base = if broadcaster { 19 } else { 15 };
        if bytes.len() < base || bytes.len() > 255 || bytes[2] > 0xef {
            return Err(ControlError::Event);
        }
        let mut event = Self {
            target: if broadcaster {
                IsoTarget::Big(bytes[2])
            } else {
                IsoTarget::BigSync(bytes[2])
            },
            status: bytes[1],
            parameters: None,
        };
        if event.status != 0 {
            return Ok(event);
        }
        let offset = if broadcaster { 10 } else { 6 };
        let timing = BigTiming {
            sync_delay_us: broadcaster.then(|| read24(&bytes[3..])),
            transport_latency_us: read24(&bytes[if broadcaster { 6 } else { 3 }..]),
            phy: broadcaster.then_some(bytes[9]),
            subevents: bytes[offset],
            burst_number: bytes[offset + 1],
            pretransmission_offset: bytes[offset + 2],
            repetition_count: bytes[offset + 3],
            max_pdu: read16(&bytes[offset + 4..]),
            iso_interval: read16(&bytes[offset + 6..]),
            handles: parse_handles(&bytes[base..], bytes[base - 1])?,
        };
        if !delay_valid(timing.transport_latency_us)
            || timing
                .sync_delay_us
                .is_some_and(|delay| !delay_valid(delay))
            || timing.phy.is_some_and(|phy| !(1..=3).contains(&phy))
            || !(1..=31).contains(&timing.subevents)
            || !(1..=7).contains(&timing.burst_number)
            || timing.pretransmission_offset > 15
            || !(1..=15).contains(&timing.repetition_count)
            || u16::from(timing.burst_number) * u16::from(timing.repetition_count)
                > u16::from(timing.subevents)
            || !(1..=251).contains(&timing.max_pdu)
            || !(4..=3200).contains(&timing.iso_interval)
            || !(1..=31).contains(&timing.handles.len())
        {
            return Err(ControlError::Event);
        }
        event.parameters = Some(EstablishedParameters::Big(timing));
        Ok(event)
    }
}

fn delay_valid(value: u32) -> bool {
    (0xea..=0x007f_ffff).contains(&value)
}

/// Per-target establishment status. Data paths remain a separate HCI operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationState {
    /// Command sent; no Command Status yet.
    AwaitingStatus,
    /// Command accepted; asynchronous establishment still outstanding.
    Pending,
    /// Controller reported successful establishment.
    Established,
    /// Command Status or the target's establishment event failed.
    Failed(u8),
}

/// Tracks one Create CIS, Accept CIS, Create BIG, or BIG Create Sync command.
#[derive(Debug)]
pub struct CreateProcedure {
    opcode: u16,
    targets: Vec<(IsoTarget, CreationState)>,
    expected_bis_count: Option<usize>,
}

impl CreateProcedure {
    /// Starts tracking a validated creation command after successful transport write.
    ///
    /// # Errors
    /// Rejects commands that do not have asynchronous establishment events.
    pub fn new(command: &ControlCommand) -> Result<Self, ControlError> {
        let parameters = command.parameters();
        let (targets, expected_bis_count) = match command.opcode() {
            0x2064 => (
                parameters[1..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pair| IsoTarget::Cis(read16(pair)))
                    .collect(),
                None,
            ),
            0x2066 => (vec![IsoTarget::Cis(read16(parameters))], None),
            0x2068 => (
                vec![IsoTarget::Big(parameters[0])],
                Some(usize::from(parameters[2])),
            ),
            0x206b => (
                vec![IsoTarget::BigSync(parameters[0])],
                Some(usize::from(parameters[23])),
            ),
            _ => return Err(ControlError::Procedure),
        };
        Ok(Self {
            opcode: command.opcode(),
            targets: targets
                .into_iter()
                .map(|target| (target, CreationState::AwaitingStatus))
                .collect(),
            expected_bis_count,
        })
    }

    /// Current state of each requested target, in command order.
    #[must_use]
    pub fn targets(&self) -> &[(IsoTarget, CreationState)] {
        &self.targets
    }

    /// Whether every target has either established or failed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.targets.iter().all(|(_, state)| {
            matches!(state, CreationState::Established | CreationState::Failed(_))
        })
    }

    /// Consumes Command Status parameters: Status, command credits, opcode LE16.
    /// The caller still owns command credits and must process that field.
    ///
    /// # Errors
    /// Rejects wrong length/opcode or repeated status. Controller failure is
    /// recorded for every target and returned without claiming establishment.
    pub fn command_status(&mut self, bytes: &[u8]) -> Result<(), ControlError> {
        if bytes.len() != 4
            || read16(&bytes[2..]) != self.opcode
            || self
                .targets
                .iter()
                .any(|(_, state)| *state != CreationState::AwaitingStatus)
        {
            return Err(ControlError::Procedure);
        }
        for (_, state) in &mut self.targets {
            *state = if bytes[0] == 0 {
                CreationState::Pending
            } else {
                CreationState::Failed(bytes[0])
            };
        }
        if bytes[0] == 0 {
            Ok(())
        } else {
            Err(ControlError::Controller(bytes[0]))
        }
    }

    /// Consumes an establishment LE Meta event, including its subevent byte.
    ///
    /// # Errors
    /// Rejects malformed, unrelated, duplicate, premature, or wrong-BIS-count
    /// events without state changes. Asynchronous failure stays in the returned
    /// event and changes only the affected target to Failed.
    pub fn established(&mut self, bytes: &[u8]) -> Result<EstablishedEvent, ControlError> {
        let event = EstablishedEvent::parse(bytes)?;
        let entry = self
            .targets
            .iter_mut()
            .find(|(target, _)| *target == event.target)
            .ok_or(ControlError::Procedure)?;
        if entry.1 != CreationState::Pending {
            return Err(ControlError::Procedure);
        }
        if let Some(EstablishedParameters::Big(timing)) = &event.parameters
            && self.expected_bis_count != Some(timing.handles.len())
        {
            return Err(ControlError::Procedure);
        }
        entry.1 = if event.status == 0 {
            CreationState::Established
        } else {
            CreationState::Failed(event.status)
        };
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "embassy")]
    fn embassy_command_adapter_writes_exact_h4_sync_and_async() {
        use bt_hci::{WriteHci, transport::WithIndicator};
        use core::{
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };

        let command = ControlCommand::create_cis(&[CisConnection {
            cis_handle: 0x100,
            acl_handle: 0x40,
        }])
        .unwrap();
        let packet = WithIndicator::new(&command);
        let expected = [1, 0x64, 0x20, 5, 1, 0, 1, 0x40, 0];
        assert_eq!(packet.size(), expected.len());
        let mut bytes = [0; 9];
        packet.write_hci(&mut bytes[..]).unwrap();
        assert_eq!(bytes, expected);
        bytes.fill(0);
        {
            let mut future = pin!(packet.write_hci_async(&mut bytes[..]));
            assert_eq!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(Ok(()))
            );
        }
        assert_eq!(bytes, expected);
        assert!(packet.write_hci(&mut bytes[..8]).is_err());
    }

    fn cig(streams: &[CisParameters]) -> CigParameters<'_> {
        CigParameters {
            id: 1,
            sdu_interval_us: [10_000, 10_000],
            worst_case_sca: 0,
            interleaved: false,
            framed: true,
            max_transport_latency_ms: [40, 40],
            streams,
        }
    }

    fn cis_event(handle: u16, status: u8) -> Vec<u8> {
        let mut bytes = vec![0x19, status];
        bytes.extend_from_slice(&handle.to_le_bytes());
        for value in [1000, 500, 20_000, 20_000] {
            put24(&mut bytes, value);
        }
        bytes.extend_from_slice(&[2, 2, 2, 1, 1, 2, 2]);
        for value in [100u16, 100, 8] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn big_parameters() -> BigParameters {
        BigParameters {
            handle: 1,
            advertising_handle: 2,
            bis_count: 2,
            sdu_interval_us: 10_000,
            max_sdu: 100,
            max_transport_latency_ms: 40,
            retransmissions: 2,
            phy: 2,
            interleaved: false,
            framed: true,
            broadcast_code: None,
        }
    }

    fn big_event() -> Vec<u8> {
        let mut bytes = vec![0x1b, 0, 1];
        put24(&mut bytes, 1000);
        put24(&mut bytes, 20_000);
        bytes.extend_from_slice(&[2, 2, 1, 0, 2, 100, 0, 8, 0, 2, 0, 1, 1, 1]);
        bytes
    }

    #[test]
    fn cig_create_and_transparent_path_match_core_wire_vectors() {
        let streams = [CisParameters {
            id: 2,
            max_sdu: [100, 0],
            phy: [2, 2],
            retransmissions: [2, 0],
        }];
        assert_eq!(
            ControlCommand::set_cig(&cig(&streams)).unwrap().encode(),
            [
                0x62, 0x20, 24, 1, 0x10, 0x27, 0, 0x10, 0x27, 0, 0, 0, 1, 40, 0, 40, 0, 1, 2, 100,
                0, 0, 0, 2, 2, 2, 0
            ]
        );
        let connections = [
            CisConnection {
                cis_handle: 0x100,
                acl_handle: 0x40,
            },
            CisConnection {
                cis_handle: 0x101,
                acl_handle: 0x40,
            },
        ];
        assert_eq!(
            ControlCommand::create_cis(&connections).unwrap().encode(),
            [0x64, 0x20, 9, 2, 0, 1, 0x40, 0, 1, 1, 0x40, 0]
        );
        assert_eq!(
            ControlCommand::setup_data_path(0x100, DataPathDirection::Input, 1000)
                .unwrap()
                .encode(),
            [0x6e, 0x20, 13, 0, 1, 0, 0, 3, 0, 0, 0, 0, 0xe8, 3, 0, 0]
        );
        assert_eq!(
            ControlCommand::remove_data_path(0x100, 3).unwrap().encode(),
            [0x6f, 0x20, 3, 0, 1, 3]
        );
        assert_eq!(
            ControlCommand::set_iso_host_support(true).encode(),
            [0x74, 0x20, 2, 32, 1]
        );
    }

    #[test]
    fn bounds_duplicate_ids_and_hci_command_size_are_checked() {
        let mut streams: Vec<_> = (0..27)
            .map(|id| CisParameters {
                id,
                max_sdu: [100, 0],
                phy: [2, 2],
                retransmissions: [255, 0],
            })
            .collect();
        assert_eq!(
            ControlCommand::set_cig(&cig(&streams)).unwrap_err(),
            ControlError::CommandLength
        );
        assert!(ControlCommand::set_cig(&cig(&streams[..26])).is_ok());
        streams[1].id = 0;
        assert_eq!(
            ControlCommand::set_cig(&cig(&streams[..2])).unwrap_err(),
            ControlError::Parameter
        );
        assert!(
            ControlCommand::setup_data_path(0xeff, DataPathDirection::Output, 4_000_000).is_ok()
        );
        assert!(ControlCommand::setup_data_path(0xf00, DataPathDirection::Input, 0).is_err());
        assert!(ControlCommand::setup_data_path(0, DataPathDirection::Input, 4_000_001).is_err());
        assert!(ControlCommand::remove_data_path(0, 0).is_err());
        let mut parameters = big_parameters();
        parameters.retransmissions = 31;
        assert!(ControlCommand::create_big(&parameters).is_err());
    }

    #[test]
    fn configured_cig_handles_do_not_hide_controller_rejection() {
        assert_eq!(
            CigConfigured::parse(&[0, 1, 2, 0, 1, 1, 1]).unwrap(),
            CigConfigured {
                id: 1,
                handles: vec![0x100, 0x101]
            }
        );
        assert_eq!(CigConfigured::parse(&[1]), Err(ControlError::Controller(1)));
        assert_eq!(
            CigConfigured::parse(&[0, 1, 2, 0, 1, 0, 1]),
            Err(ControlError::Event)
        );
        assert_eq!(
            CigConfigured::parse(&[0, 1, 1, 0, 0xf]),
            Err(ControlError::Event)
        );
    }

    #[test]
    fn command_acceptance_is_not_establishment_and_each_cis_completes_separately() {
        let command = ControlCommand::create_cis(&[
            CisConnection {
                cis_handle: 0x100,
                acl_handle: 0x40,
            },
            CisConnection {
                cis_handle: 0x101,
                acl_handle: 0x40,
            },
        ])
        .unwrap();
        let mut procedure = CreateProcedure::new(&command).unwrap();
        assert_eq!(
            procedure.established(&cis_event(0x100, 0)),
            Err(ControlError::Procedure)
        );
        procedure.command_status(&[0, 1, 0x64, 0x20]).unwrap();
        assert!(!procedure.is_complete());
        procedure.established(&cis_event(0x101, 0x3e)).unwrap();
        assert!(!procedure.is_complete());
        procedure.established(&cis_event(0x100, 0)).unwrap();
        assert!(procedure.is_complete());
        assert_eq!(
            procedure.targets(),
            [
                (IsoTarget::Cis(0x100), CreationState::Established),
                (IsoTarget::Cis(0x101), CreationState::Failed(0x3e))
            ]
        );
        assert_eq!(
            procedure.established(&cis_event(0x100, 0)),
            Err(ControlError::Procedure)
        );
        let mut failed = CreateProcedure::new(&command).unwrap();
        assert_eq!(
            failed.command_status(&[1, 1, 0x64, 0x20]),
            Err(ControlError::Controller(1))
        );
        assert!(failed.is_complete());
        assert_eq!(failed.targets()[0].1, CreationState::Failed(1));
    }

    #[test]
    fn big_source_and_receiver_commands_preserve_lists_and_encryption() {
        let command = ControlCommand::create_big(&big_parameters()).unwrap();
        assert_eq!(command.parameters().len(), 31);
        assert_eq!(&command.parameters()[14..], &[0; 17]);
        let mut procedure = CreateProcedure::new(&command).unwrap();
        procedure.command_status(&[0, 1, 0x68, 0x20]).unwrap();
        let event = procedure.established(&big_event()).unwrap();
        assert!(
            matches!(event.parameters, Some(EstablishedParameters::Big(BigTiming { handles, .. })) if handles == [0x100, 0x101])
        );
        assert!(procedure.is_complete());
        let parameters = BigSyncParameters {
            handle: 1,
            sync_handle: 0x40,
            broadcast_code: Some([0xa5; 16]),
            max_subevents: 0,
            timeout: 100,
            bis_indices: &[1, 3],
        };
        let sync = ControlCommand::sync_big(&parameters).unwrap();
        assert_eq!(
            sync.parameters(),
            [
                1, 0x40, 0, 1, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5,
                0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0, 100, 0, 2, 1, 3
            ]
        );
        let mut procedure = CreateProcedure::new(&sync).unwrap();
        procedure.command_status(&[0, 1, 0x6b, 0x20]).unwrap();
        let original = big_event();
        let mut synced = vec![0x1d, 0, 1];
        synced.extend_from_slice(&original[6..9]);
        synced.extend_from_slice(&original[10..]);
        procedure.established(&synced).unwrap();
        assert!(procedure.is_complete());
        assert!(
            ControlCommand::sync_big(&BigSyncParameters {
                bis_indices: &[3, 1],
                ..parameters
            })
            .is_err()
        );
    }

    #[test]
    fn malformed_events_never_advance_procedure_and_failure_ignores_timing() {
        let command = ControlCommand::accept_cis(0x100).unwrap();
        let mut procedure = CreateProcedure::new(&command).unwrap();
        assert_eq!(
            procedure.command_status(&[0, 1, 0x64, 0x20]),
            Err(ControlError::Procedure)
        );
        procedure.command_status(&[0, 1, 0x66, 0x20]).unwrap();
        let valid = cis_event(0x100, 0);
        for length in 0..valid.len() {
            assert!(procedure.established(&valid[..length]).is_err());
        }
        let mut bad = valid;
        bad[16] = 4;
        assert_eq!(procedure.established(&bad), Err(ControlError::Event));
        assert!(!procedure.is_complete());
        bad[1] = 0x3e;
        assert!(procedure.established(&bad).unwrap().parameters.is_none());
        assert_eq!(procedure.targets()[0].1, CreationState::Failed(0x3e));
    }
}
