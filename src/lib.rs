//! Parser and exact rebuilder for CYW43 Bluetooth firmware record blobs.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use cyw43_controller_proto as proto;

pub mod bap;
#[cfg(feature = "embassy")]
pub mod embassy;
pub mod iso;
pub mod iso_control;
pub mod iso_credit;
pub mod iso_sdu;
#[cfg(feature = "lc3")]
pub mod lc3;

use alloc::{vec, vec::Vec};
use core::error::Error;
use core::fmt;

/// Parsed CYW43 Bluetooth firmware blob.
#[derive(Debug, PartialEq, Eq)]
pub struct Firmware<'a> {
    version: &'a str,
    records: Vec<Record<'a>>,
}

impl<'a> Firmware<'a> {
    /// Parses a complete firmware blob.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] when the header or any declared record is invalid.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ParseError> {
        let (&version_len, rest) = bytes.split_first().ok_or(ParseError::Empty)?;
        let version_len = usize::from(version_len);
        let version_bytes = rest
            .get(..version_len)
            .ok_or(ParseError::Truncated { offset: 1 })?;
        let Some((&record_count, mut records_bytes)) =
            rest.get(version_len..).and_then(<[u8]>::split_first)
        else {
            return Err(ParseError::Truncated {
                offset: 1 + version_len,
            });
        };

        let version_bytes = version_bytes
            .strip_suffix(&[0])
            .ok_or(ParseError::UnterminatedVersion)?;
        let version = core::str::from_utf8(version_bytes).map_err(ParseError::InvalidVersion)?;
        let mut records = Vec::with_capacity(usize::from(record_count));
        let mut offset = version_len + 2;

        for _ in 0..record_count {
            let header = records_bytes
                .get(..4)
                .ok_or(ParseError::Truncated { offset })?;
            let data_len = usize::from(header[0]);
            let record_len = 4 + data_len;
            let encoded_record = records_bytes
                .get(..record_len)
                .ok_or(ParseError::Truncated { offset })?;
            records.push(Record {
                address: u16::from_be_bytes([header[1], header[2]]),
                kind: RecordKind::parse(header[3], &encoded_record[4..], offset + 4)?,
            });
            records_bytes = &records_bytes[record_len..];
            offset += record_len;
        }

        if !records_bytes.is_empty() {
            return Err(ParseError::TrailingBytes {
                offset,
                count: records_bytes.len(),
            });
        }

        Ok(Self { version, records })
    }

    /// Returns firmware version string.
    #[must_use]
    pub fn version(&self) -> &str {
        self.version
    }

    /// Returns source records in original order.
    #[must_use]
    pub fn records(&self) -> &[Record<'a>] {
        &self.records
    }

    /// Rebuilds original record container.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if version or record counts exceed format limits.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let version_len = self
            .version
            .len()
            .checked_add(1)
            .and_then(|len| u8::try_from(len).ok())
            .ok_or(EncodeError::VersionTooLong)?;
        let record_count =
            u8::try_from(self.records.len()).map_err(|_| EncodeError::TooManyRecords)?;
        let mut bytes = Vec::new();
        bytes.push(version_len);
        bytes.extend_from_slice(self.version.as_bytes());
        bytes.push(0);
        bytes.push(record_count);
        for record in &self.records {
            record.encode(&mut bytes)?;
        }
        Ok(bytes)
    }

    /// Resolves data records into sparse address ranges.
    #[must_use]
    pub fn segments(&self) -> Vec<Segment<'a>> {
        let mut base = 0_u32;
        let mut segments: Vec<Segment<'a>> = Vec::new();

        for record in &self.records {
            match record.kind {
                RecordKind::Data(data) => {
                    let address = base + u32::from(record.address);
                    if let Some(last) = segments.last_mut()
                        && last.end_address() == address
                    {
                        last.parts.push(data);
                    } else {
                        segments.push(Segment {
                            address,
                            parts: vec![data],
                        });
                    }
                }
                RecordKind::ExtendedSegment(value) => base = u32::from(value) << 4,
                RecordKind::ExtendedLinear(value) => base = u32::from(value) << 16,
                RecordKind::Absolute32(value) => base = value,
                RecordKind::EndOfFile | RecordKind::Unknown { .. } => {}
            }
        }

        segments
    }

    /// Recovers compact ROM-to-Patchram hook records embedded in configuration data.
    #[must_use]
    pub fn patch_hooks(&self) -> Vec<PatchHook> {
        let segments = self.segments();
        let code_ranges: Vec<_> = segments
            .iter()
            .filter(|segment| segment.address() < 0x0020_0000)
            .map(|segment| (segment.address(), segment.end_address()))
            .collect();
        let mut longest = Vec::new();

        for segment in &segments {
            let bytes = segment.to_vec();
            for start in 0..bytes.len().saturating_sub(7) {
                let mut hooks = Vec::new();
                let mut offset = start;
                while let Some(hook) = parse_patch_hook(&bytes[offset..], &code_ranges) {
                    hooks.push(hook);
                    offset += 8;
                    if offset + 8 > bytes.len() {
                        break;
                    }
                }
                if hooks.len() > longest.len() {
                    longest = hooks;
                }
            }
        }

        longest
    }

    /// Builds firmware that traces entries into one existing Patchram hook.
    ///
    /// The wrapper is appended immediately after the hook's current code segment,
    /// increments a private 32-bit counter, captures `r0` through `r3`, then
    /// tail-branches to the stock hook.
    ///
    /// # Errors
    ///
    /// Returns [`HookCounterError`] when the hook is missing or ambiguous, no safe
    /// append location exists, the branch cannot be encoded, or the container has
    /// no final end record.
    pub fn with_hook_counter(
        &self,
        rom_pointer: u32,
    ) -> Result<HookCounterImage, HookCounterError> {
        let matching: Vec<_> = self
            .patch_hooks()
            .into_iter()
            .filter(|hook| hook.rom_pointer == rom_pointer)
            .collect();
        let hook = match matching.as_slice() {
            [] => return Err(HookCounterError::HookNotFound(rom_pointer)),
            [hook] => hook,
            _ => return Err(HookCounterError::AmbiguousHook(rom_pointer)),
        };
        let original_patch_address = hook.patch_pointer & !1;
        let code_segment = self
            .segments()
            .into_iter()
            .find(|segment| {
                (segment.address()..segment.end_address()).contains(&original_patch_address)
            })
            .ok_or(HookCounterError::PatchOutsideCode(hook.patch_pointer))?;
        let wrapper_address = code_segment
            .end_address()
            .checked_add(3)
            .map(|address| address & !3)
            .ok_or(HookCounterError::AddressOverflow)?;
        let counter_address = wrapper_address
            .checked_add(28)
            .ok_or(HookCounterError::AddressOverflow)?;
        let wrapper_end = wrapper_address
            .checked_add(48)
            .ok_or(HookCounterError::AddressOverflow)?;

        if self.segments().iter().any(|segment| {
            wrapper_address < segment.end_address() && segment.address() < wrapper_end
        }) {
            return Err(HookCounterError::AppendOverlap(wrapper_address));
        }

        let branch = encode_thumb_b_w(wrapper_address + 20, original_patch_address)?;
        let mut wrapper = vec![
            0x3f, 0xb4, 0x05, 0x4c, 0x25, 0x68, 0x01, 0x35, 0x25, 0x60, 0x60, 0x60, 0xa1, 0x60,
            0xe2, 0x60, 0x23, 0x61, 0x3f, 0xbc,
        ];
        wrapper.extend_from_slice(&branch);
        wrapper.extend_from_slice(&counter_address.to_le_bytes());
        wrapper.extend_from_slice(&[0_u8; 20]);

        let new_hook = PatchHook {
            rom_pointer: hook.rom_pointer,
            patch_pointer: wrapper_address | 1,
            kind: hook.kind,
        };
        let bytes = self.encode_hook_counter(hook, &new_hook, &wrapper)?;

        Ok(HookCounterImage {
            bytes,
            wrapper_address,
            counter_address,
            original_patch_pointer: hook.patch_pointer,
        })
    }

    fn encode_hook_counter(
        &self,
        old_hook: &PatchHook,
        new_hook: &PatchHook,
        wrapper: &[u8],
    ) -> Result<Vec<u8>, HookCounterError> {
        let old_entry = encode_patch_hook(old_hook);
        let new_entry = encode_patch_hook(new_hook);
        let wrapper_address = new_hook.patch_pointer & !1;
        let end_index = self
            .records
            .iter()
            .position(|record| matches!(record.kind, RecordKind::EndOfFile))
            .ok_or(HookCounterError::MissingEndRecord)?;
        if end_index + 1 != self.records.len() {
            return Err(HookCounterError::EndRecordNotLast);
        }
        let record_count = self
            .records
            .len()
            .checked_add(2)
            .and_then(|count| u8::try_from(count).ok())
            .ok_or(HookCounterError::TooManyRecords)?;
        let version_len = self
            .version
            .len()
            .checked_add(1)
            .and_then(|len| u8::try_from(len).ok())
            .ok_or(HookCounterError::VersionTooLong)?;
        let mut replacements = 0_usize;
        let mut bytes = Vec::new();
        bytes.push(version_len);
        bytes.extend_from_slice(self.version.as_bytes());
        bytes.push(0);
        bytes.push(record_count);

        for (index, record) in self.records.iter().enumerate() {
            if index == end_index {
                push_raw_record(
                    &mut bytes,
                    0,
                    4,
                    &u16::try_from(wrapper_address >> 16)
                        .map_err(|_| HookCounterError::AddressOverflow)?
                        .to_be_bytes(),
                )?;
                push_raw_record(
                    &mut bytes,
                    u16::try_from(wrapper_address & 0xffff)
                        .map_err(|_| HookCounterError::AddressOverflow)?,
                    0,
                    wrapper,
                )?;
            }

            match record.kind {
                RecordKind::Data(data) => {
                    let mut patched = data.to_vec();
                    for window_start in 0..=patched.len().saturating_sub(old_entry.len()) {
                        if patched[window_start..].starts_with(&old_entry) {
                            patched[window_start..window_start + new_entry.len()]
                                .copy_from_slice(&new_entry);
                            replacements += 1;
                        }
                    }
                    push_raw_record(&mut bytes, record.address, 0, &patched)?;
                }
                _ => record
                    .encode(&mut bytes)
                    .map_err(HookCounterError::Encode)?,
            }
        }

        if replacements != 1 {
            return Err(HookCounterError::HookEncodingOccurrences(replacements));
        }
        Ok(bytes)
    }
}

/// Generated hook-entry telemetry firmware and its controller addresses.
#[derive(Debug, PartialEq, Eq)]
pub struct HookCounterImage {
    bytes: Vec<u8>,
    wrapper_address: u32,
    counter_address: u32,
    original_patch_pointer: u32,
}

impl HookCounterImage {
    /// Returns encoded firmware bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns appended Thumb wrapper address without the Thumb bit.
    #[must_use]
    pub const fn wrapper_address(&self) -> u32 {
        self.wrapper_address
    }

    /// Returns telemetry address: counter followed by the last `r0` through `r3`.
    #[must_use]
    pub const fn counter_address(&self) -> u32 {
        self.counter_address
    }

    /// Returns original encoded Patchram Thumb pointer.
    #[must_use]
    pub const fn original_patch_pointer(&self) -> u32 {
        self.original_patch_pointer
    }
}

/// One encoded ROM replacement installed by the stock firmware.
#[derive(Debug, PartialEq, Eq)]
pub struct PatchHook {
    rom_pointer: u32,
    patch_pointer: u32,
    kind: u8,
}

impl PatchHook {
    /// Returns encoded Thumb pointer into mask ROM.
    #[must_use]
    pub const fn rom_pointer(&self) -> u32 {
        self.rom_pointer
    }

    /// Returns encoded Thumb pointer into Patchram.
    #[must_use]
    pub const fn patch_pointer(&self) -> u32 {
        self.patch_pointer
    }

    /// Returns vendor hook kind. Meaning remains unknown; observed values are 1 and 2.
    #[must_use]
    pub const fn kind(&self) -> u8 {
        self.kind
    }
}

/// One source record.
#[derive(Debug, PartialEq, Eq)]
pub struct Record<'a> {
    address: u16,
    kind: RecordKind<'a>,
}

impl<'a> Record<'a> {
    /// Returns 16-bit record address field.
    #[must_use]
    pub const fn address(&self) -> u16 {
        self.address
    }

    /// Returns decoded record kind.
    #[must_use]
    pub const fn kind(&self) -> &RecordKind<'a> {
        &self.kind
    }

    fn encode(&self, output: &mut Vec<u8>) -> Result<(), EncodeError> {
        let (type_id, data) = self.kind.encoded();
        output.push(u8::try_from(data.len()).map_err(|_| EncodeError::RecordTooLong)?);
        output.extend_from_slice(&self.address.to_be_bytes());
        output.push(type_id);
        output.extend_from_slice(&data);
        Ok(())
    }
}

/// Decoded record payload.
#[derive(Debug, PartialEq, Eq)]
pub enum RecordKind<'a> {
    /// Bytes copied to current base plus record address.
    Data(&'a [u8]),
    /// End marker.
    EndOfFile,
    /// Base address encoded in 16-byte paragraphs.
    ExtendedSegment(u16),
    /// Base address encoded as upper 16 bits.
    ExtendedLinear(u16),
    /// Full 32-bit base address.
    Absolute32(u32),
    /// Preserved vendor record with unknown semantics.
    Unknown { type_id: u8, data: &'a [u8] },
}

impl<'a> RecordKind<'a> {
    fn parse(type_id: u8, data: &'a [u8], offset: usize) -> Result<Self, ParseError> {
        match type_id {
            0 => Ok(Self::Data(data)),
            1 if data.is_empty() => Ok(Self::EndOfFile),
            2 => Ok(Self::ExtendedSegment(parse_u16(data, offset)?)),
            4 => Ok(Self::ExtendedLinear(parse_u16(data, offset)?)),
            5 => Ok(Self::Absolute32(parse_u32(data, offset)?)),
            _ => Ok(Self::Unknown { type_id, data }),
        }
    }

    fn encoded(&self) -> (u8, Vec<u8>) {
        match self {
            Self::Data(data) => (0, data.to_vec()),
            Self::EndOfFile => (1, Vec::new()),
            Self::ExtendedSegment(value) => (2, value.to_be_bytes().to_vec()),
            Self::ExtendedLinear(value) => (4, value.to_be_bytes().to_vec()),
            Self::Absolute32(value) => (5, value.to_be_bytes().to_vec()),
            Self::Unknown { type_id, data } => (*type_id, data.to_vec()),
        }
    }
}

/// Contiguous sparse firmware region.
#[derive(Debug, PartialEq, Eq)]
pub struct Segment<'a> {
    address: u32,
    parts: Vec<&'a [u8]>,
}

impl Segment<'_> {
    /// Returns controller-relative start address.
    #[must_use]
    pub const fn address(&self) -> u32 {
        self.address
    }

    /// Returns exclusive controller-relative end address.
    #[must_use]
    pub fn end_address(&self) -> u32 {
        self.address + u32::try_from(self.len()).unwrap_or(u32::MAX)
    }

    /// Returns region byte length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.iter().map(|part| part.len()).sum()
    }

    /// Returns whether region contains no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|part| part.is_empty())
    }

    /// Copies region into one contiguous buffer.
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len());
        for part in &self.parts {
            bytes.extend_from_slice(part);
        }
        bytes
    }
}

/// Firmware parse failure.
#[derive(Debug)]
pub enum ParseError {
    /// Input has no header byte.
    Empty,
    /// Input ends before a declared field or record.
    Truncated { offset: usize },
    /// Version field lacks required null terminator.
    UnterminatedVersion,
    /// Version field is not UTF-8.
    InvalidVersion(core::str::Utf8Error),
    /// Bytes remain after declared record count.
    TrailingBytes { offset: usize, count: usize },
    /// Known address record has wrong payload size.
    InvalidAddressRecord { offset: usize, expected: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "firmware blob is empty"),
            Self::Truncated { offset } => {
                write!(formatter, "firmware blob truncated at offset {offset:#x}")
            }
            Self::UnterminatedVersion => write!(formatter, "version field is not null terminated"),
            Self::InvalidVersion(error) => {
                write!(formatter, "version field is invalid UTF-8: {error}")
            }
            Self::TrailingBytes { offset, count } => {
                write!(formatter, "{count} trailing bytes after offset {offset:#x}")
            }
            Self::InvalidAddressRecord { offset, expected } => {
                write!(
                    formatter,
                    "address record at {offset:#x} must contain {expected} bytes"
                )
            }
        }
    }
}

impl Error for ParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidVersion(error) => Some(error),
            _ => None,
        }
    }
}

/// Firmware encode failure.
#[derive(Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// Null-terminated version exceeds 255 bytes.
    VersionTooLong,
    /// Container exceeds 255 records.
    TooManyRecords,
    /// Record payload exceeds 255 bytes.
    RecordTooLong,
}

/// Failure while generating a hook-entry telemetry image.
#[derive(Debug, PartialEq, Eq)]
pub enum HookCounterError {
    /// Requested ROM hook is absent.
    HookNotFound(u32),
    /// More than one decoded hook has the requested ROM pointer.
    AmbiguousHook(u32),
    /// Existing Patchram destination is not inside a loaded code segment.
    PatchOutsideCode(u32),
    /// Address arithmetic exceeded the 32-bit controller address space.
    AddressOverflow,
    /// Appended wrapper would overlap an existing firmware segment.
    AppendOverlap(u32),
    /// Container has no end record.
    MissingEndRecord,
    /// Data follows the end record, so appending would change loader semantics.
    EndRecordNotLast,
    /// Container cannot hold two more records.
    TooManyRecords,
    /// Version cannot be re-encoded.
    VersionTooLong,
    /// Compact hook encoding did not occur exactly once in source records.
    HookEncodingOccurrences(usize),
    /// Thumb branch target is outside the B.W range or is not halfword aligned.
    BranchOutOfRange { source: u32, target: u32 },
    /// Existing record could not be encoded.
    Encode(EncodeError),
}

impl fmt::Display for HookCounterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HookNotFound(pointer) => write!(formatter, "ROM hook {pointer:#010x} not found"),
            Self::AmbiguousHook(pointer) => {
                write!(formatter, "ROM hook {pointer:#010x} is ambiguous")
            }
            Self::PatchOutsideCode(pointer) => {
                write!(
                    formatter,
                    "Patchram pointer {pointer:#010x} is outside code"
                )
            }
            Self::AddressOverflow => write!(formatter, "controller address overflow"),
            Self::AppendOverlap(address) => {
                write!(
                    formatter,
                    "wrapper at {address:#010x} overlaps firmware data"
                )
            }
            Self::MissingEndRecord => write!(formatter, "firmware has no end record"),
            Self::EndRecordNotLast => write!(formatter, "firmware end record is not last"),
            Self::TooManyRecords => write!(formatter, "firmware cannot hold telemetry records"),
            Self::VersionTooLong => write!(formatter, "firmware version is too long"),
            Self::HookEncodingOccurrences(count) => {
                write!(
                    formatter,
                    "hook encoding occurs {count} times instead of once"
                )
            }
            Self::BranchOutOfRange { source, target } => write!(
                formatter,
                "Thumb branch from {source:#010x} to {target:#010x} is out of range"
            ),
            Self::Encode(error) => write!(formatter, "firmware encode failed: {error}"),
        }
    }
}

impl Error for HookCounterError {}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionTooLong => write!(formatter, "version exceeds 254 bytes"),
            Self::TooManyRecords => write!(formatter, "firmware exceeds 255 records"),
            Self::RecordTooLong => write!(formatter, "record payload exceeds 255 bytes"),
        }
    }
}

impl Error for EncodeError {}

/// Converts a Broadcom HCD patch stream into Pico SDIO firmware records.
///
/// # Errors
///
/// Returns [`HcdError`] for malformed or unsupported HCD commands.
pub fn hcd_to_firmware(version: &str, hcd: &[u8]) -> Result<Vec<u8>, HcdError> {
    let version_len = version
        .len()
        .checked_add(1)
        .and_then(|len| u8::try_from(len).ok())
        .ok_or(HcdError::VersionTooLong)?;
    let mut records = Vec::new();
    let mut cursor = 0;
    let mut upper = None;

    while cursor < hcd.len() {
        let command_start = cursor;
        let header_end = cursor.checked_add(3).ok_or(HcdError::Truncated(cursor))?;
        let header = hcd
            .get(cursor..header_end)
            .ok_or(HcdError::Truncated(cursor))?;
        let &[opcode_low, opcode_high, length] = header else {
            return Err(HcdError::Truncated(cursor));
        };
        let opcode = u16::from_le_bytes([opcode_low, opcode_high]);
        let length = usize::from(length);
        let command_end = header_end
            .checked_add(length)
            .ok_or(HcdError::Truncated(cursor))?;
        let payload = hcd
            .get(header_end..command_end)
            .ok_or(HcdError::Truncated(cursor))?;
        cursor = command_end;

        match opcode {
            0xfc4c => {
                let source = payload
                    .get(..4)
                    .ok_or(HcdError::InvalidWrite(command_start))?;
                let address_bytes: [u8; 4] = source
                    .try_into()
                    .map_err(|_| HcdError::InvalidWrite(command_start))?;
                let mut address = u32::from_le_bytes(address_bytes);
                let mut data = payload
                    .get(4..)
                    .ok_or(HcdError::InvalidWrite(command_start))?;
                while !data.is_empty() {
                    let [address_low, address_high, _, _] = address.to_le_bytes();
                    let address_low = u16::from_le_bytes([address_low, address_high]);
                    let next_boundary = 0x1_0000 - usize::from(address_low);
                    let chunk_len = data.len().min(251).min(next_boundary);
                    let address_upper = u16::try_from(address >> 16)
                        .map_err(|_| HcdError::AddressOutOfRange(address))?;
                    if upper != Some(address_upper) {
                        push_record(&mut records, 0, 4, &address_upper.to_be_bytes())?;
                        upper = Some(address_upper);
                    }
                    let chunk = data
                        .get(..chunk_len)
                        .ok_or(HcdError::AddressOutOfRange(address))?;
                    push_record(&mut records, address_low, 0, chunk)?;
                    let chunk_len = u32::try_from(chunk_len)
                        .map_err(|_| HcdError::AddressOutOfRange(address))?;
                    address = address
                        .checked_add(chunk_len)
                        .ok_or(HcdError::AddressOutOfRange(address))?;
                    data = data
                        .get(
                            usize::try_from(chunk_len)
                                .map_err(|_| HcdError::AddressOutOfRange(address))?..,
                        )
                        .ok_or(HcdError::AddressOutOfRange(address))?;
                }
            }
            0xfc4e if payload == [0xff; 4] => {
                // ponytail: Pico W SDIO loader cannot execute HCD Launch RAM.
                // These CYW43439 boot records match every shipped Pico SDIO image.
                if upper != Some(0x20) {
                    push_record(&mut records, 0, 4, &0x20_u16.to_be_bytes())?;
                    upper = Some(0x20);
                }
                push_record(&mut records, 0x38d4, 0, &0x0021_e000_u32.to_le_bytes())?;
                push_record(&mut records, 0x4164, 0, &0x0005_f7ed_u32.to_le_bytes())?;
            }
            _ => return Err(HcdError::UnsupportedOpcode(opcode)),
        }
    }

    push_record(&mut records, 0, 1, &[])?;
    let count = u8::try_from(records.len()).map_err(|_| HcdError::TooManyRecords)?;
    let mut output = Vec::new();
    output.push(version_len);
    output.extend_from_slice(version.as_bytes());
    output.push(0);
    output.push(count);
    for record in records {
        output.extend_from_slice(&record);
    }
    Ok(output)
}

fn push_record(
    records: &mut Vec<Vec<u8>>,
    address: u16,
    kind: u8,
    data: &[u8],
) -> Result<(), HcdError> {
    let length = u8::try_from(data.len()).map_err(|_| HcdError::RecordTooLong)?;
    let mut record = Vec::with_capacity(data.len() + 4);
    record.push(length);
    record.extend_from_slice(&address.to_be_bytes());
    record.push(kind);
    record.extend_from_slice(data);
    records.push(record);
    Ok(())
}

/// HCD conversion failure.
#[derive(Debug, PartialEq, Eq)]
pub enum HcdError {
    VersionTooLong,
    Truncated(usize),
    InvalidWrite(usize),
    UnsupportedOpcode(u16),
    AddressOutOfRange(u32),
    RecordTooLong,
    TooManyRecords,
}

impl fmt::Display for HcdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionTooLong => write!(formatter, "version exceeds 254 bytes"),
            Self::Truncated(offset) => write!(formatter, "truncated HCD command at {offset:#x}"),
            Self::InvalidWrite(offset) => {
                write!(formatter, "invalid Write RAM command at {offset:#x}")
            }
            Self::UnsupportedOpcode(opcode) => {
                write!(formatter, "unsupported HCD opcode {opcode:#06x}")
            }
            Self::AddressOutOfRange(address) => {
                write!(formatter, "address {address:#010x} exceeds 32 bits")
            }
            Self::RecordTooLong => write!(formatter, "record payload exceeds 255 bytes"),
            Self::TooManyRecords => write!(formatter, "firmware exceeds 255 records"),
        }
    }
}

impl Error for HcdError {}

fn parse_u16(data: &[u8], offset: usize) -> Result<u16, ParseError> {
    let bytes: [u8; 2] = data
        .try_into()
        .map_err(|_| ParseError::InvalidAddressRecord {
            offset,
            expected: 2,
        })?;
    Ok(u16::from_be_bytes(bytes))
}

fn parse_u32(data: &[u8], offset: usize) -> Result<u32, ParseError> {
    let bytes: [u8; 4] = data
        .try_into()
        .map_err(|_| ParseError::InvalidAddressRecord {
            offset,
            expected: 4,
        })?;
    Ok(u32::from_be_bytes(bytes))
}

fn parse_patch_hook(bytes: &[u8], code_ranges: &[(u32, u32)]) -> Option<PatchHook> {
    let entry = bytes.get(..8)?;
    let patch_pointer = (u32::from(u16::from_le_bytes([entry[0], entry[1]])) << 16)
        | u32::from(u16::from_le_bytes([entry[6], entry[7]]));
    let rom_pointer =
        u32::from(entry[2]) | (u32::from(entry[3]) << 8) | (u32::from(entry[4]) << 16);
    let kind = entry[5];
    let patch_address = patch_pointer & !1;
    let points_into_code = code_ranges
        .iter()
        .any(|&(start, end)| (start..end).contains(&patch_address));

    (rom_pointer & 1 != 0 && patch_pointer & 1 != 0 && matches!(kind, 1 | 2) && points_into_code)
        .then_some(PatchHook {
            rom_pointer,
            patch_pointer,
            kind,
        })
}

fn encode_patch_hook(hook: &PatchHook) -> [u8; 8] {
    let patch_high = u16::try_from(hook.patch_pointer >> 16).unwrap_or_default();
    let patch_low = u16::try_from(hook.patch_pointer & 0xffff).unwrap_or_default();
    let rom = hook.rom_pointer.to_le_bytes();
    let mut bytes = [0_u8; 8];
    bytes[0..2].copy_from_slice(&patch_high.to_le_bytes());
    bytes[2..5].copy_from_slice(&rom[..3]);
    bytes[5] = hook.kind;
    bytes[6..8].copy_from_slice(&patch_low.to_le_bytes());
    bytes
}

fn encode_thumb_b_w(source: u32, target: u32) -> Result<[u8; 4], HookCounterError> {
    let delta = i64::from(target) - i64::from(source + 4);
    if delta & 1 != 0 || !(-0x0100_0000..0x0100_0000).contains(&delta) {
        return Err(HookCounterError::BranchOutOfRange { source, target });
    }

    let immediate = u32::try_from(delta & 0x01ff_ffff).unwrap_or_default();
    let sign = (immediate >> 24) & 1;
    let i1 = (immediate >> 23) & 1;
    let i2 = (immediate >> 22) & 1;
    let j1 = (!(i1 ^ sign)) & 1;
    let j2 = (!(i2 ^ sign)) & 1;
    let first = 0xf000 | (sign << 10) | ((immediate >> 12) & 0x03ff);
    let second = 0x9000 | (j1 << 13) | (j2 << 11) | ((immediate >> 1) & 0x07ff);
    let mut bytes = [0_u8; 4];
    bytes[..2].copy_from_slice(&u16::try_from(first).unwrap_or_default().to_le_bytes());
    bytes[2..].copy_from_slice(&u16::try_from(second).unwrap_or_default().to_le_bytes());
    Ok(bytes)
}

fn push_raw_record(
    output: &mut Vec<u8>,
    address: u16,
    type_id: u8,
    data: &[u8],
) -> Result<(), HookCounterError> {
    output.push(u8::try_from(data.len()).map_err(|_| HookCounterError::TooManyRecords)?);
    output.extend_from_slice(&address.to_be_bytes());
    output.push(type_id);
    output.extend_from_slice(data);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Firmware, Record, RecordKind, encode_thumb_b_w, hcd_to_firmware, parse_patch_hook,
    };
    use alloc::vec;

    const FIXTURE: &[u8] = &[
        4, b'v', b'1', b'0', 0, 4, 2, 0, 0, 4, 0, 0x21, 3, 0xe0, 0, 0, 1, 2, 3, 2, 0xe0, 3, 0, 4,
        5, 0, 0, 0, 1,
    ];

    #[test]
    fn encode_should_reproduce_original_blob() {
        let firmware = Firmware::parse(FIXTURE).expect("fixture must parse");

        assert_eq!(firmware.encode().expect("fixture must encode"), FIXTURE);
    }

    #[test]
    fn segments_should_merge_adjacent_data_records() {
        let firmware = Firmware::parse(FIXTURE).expect("fixture must parse");

        assert_eq!(firmware.segments()[0].to_vec(), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn hcd_conversion_should_preserve_write_addresses() {
        let hcd = [
            0x4c, 0xfc, 6, 0xfe, 0xff, 0x21, 0, 1, 2, 0x4e, 0xfc, 4, 0xff, 0xff, 0xff, 0xff,
        ];
        let encoded = hcd_to_firmware("test", &hcd).expect("HCD must convert");
        let firmware = Firmware::parse(&encoded).expect("converted firmware must parse");
        let segments = firmware.segments();

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].address(), 0x0021_fffe);
        assert_eq!(segments[0].to_vec(), [1, 2]);
    }

    #[test]
    fn patch_hook_should_decode_compact_pointer_pair() {
        let hook = parse_patch_hook(
            &[0x0d, 0x00, 0x79, 0x2d, 0x07, 0x01, 0xdd, 0x03],
            &[(0x000d_0200, 0x000d_0f90)],
        )
        .expect("valid hook must decode");

        assert_eq!(hook.rom_pointer(), 0x0007_2d79);
        assert_eq!(hook.patch_pointer(), 0x000d_03dd);
        assert_eq!(hook.kind(), 1);
    }

    #[test]
    fn thumb_branch_should_match_stock_tail_branch() {
        assert_eq!(
            encode_thumb_b_w(0x000d_0662, 0x0007_4360).expect("branch must encode"),
            [0xa3, 0xf7, 0x7d, 0xbe]
        );
    }

    #[test]
    fn hook_counter_should_append_wrapper_and_repoint_hook() {
        let hook = [0x0d, 0x00, 0x19, 0x44, 0x07, 0x01, 0x01, 0x02];
        let code = [0_u8; 32];
        let firmware = Firmware {
            version: "test",
            records: vec![
                Record {
                    address: 0,
                    kind: RecordKind::ExtendedLinear(0x000d),
                },
                Record {
                    address: 0x0200,
                    kind: RecordKind::Data(&code),
                },
                Record {
                    address: 0,
                    kind: RecordKind::ExtendedLinear(0x0021),
                },
                Record {
                    address: 0xf000,
                    kind: RecordKind::Data(&hook),
                },
                Record {
                    address: 0,
                    kind: RecordKind::EndOfFile,
                },
            ],
        };

        let image = firmware
            .with_hook_counter(0x0007_4419)
            .expect("counter image must build");
        let reparsed = Firmware::parse(image.bytes()).expect("counter image must parse");
        let patched_hook = reparsed
            .patch_hooks()
            .into_iter()
            .find(|entry| entry.rom_pointer() == 0x0007_4419)
            .expect("patched hook must remain discoverable");

        assert_eq!(image.wrapper_address(), 0x000d_0220);
        assert_eq!(image.counter_address(), 0x000d_023c);
        assert_eq!(image.original_patch_pointer(), 0x000d_0201);
        assert_eq!(patched_hook.patch_pointer(), 0x000d_0221);
        assert_eq!(
            reparsed
                .segments()
                .into_iter()
                .find(|segment| segment.address() == 0x000d_0220)
                .expect("wrapper segment must exist")
                .to_vec()[..20],
            [
                0x3f, 0xb4, 0x05, 0x4c, 0x25, 0x68, 0x01, 0x35, 0x25, 0x60, 0x60, 0x60, 0xa1, 0x60,
                0xe2, 0x60, 0x23, 0x61, 0x3f, 0xbc,
            ]
        );
    }
}
