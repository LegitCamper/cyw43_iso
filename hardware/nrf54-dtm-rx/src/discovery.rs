//! Bounded diagnostic receiver for one cleartext, unframed, NSE=BN=IRC=1 BIS.
use cyw43_controller_proto::{
    bis::{BisParameters, validate_access_address},
    channel::IsoChannels,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Primary,
    Auxiliary,
    Periodic,
    Bis,
}

#[derive(Clone, Copy, Debug)]
pub struct Radio {
    pub address: u32,
    pub crc: u32,
    pub channel: u8,
}

#[derive(Debug)]
pub struct Discovery {
    pub stage: Stage,
    pub radio: Radio,
    pub broadcast_id: u32,
    pub sample_rate: u32,
    pub frame_us: u32,
    pub frame_bytes: u16,
    pub bis_counter: u16,
    pub periodic_counter: u16,
    expected: Option<(u32, u32)>,
    adi: u16,
    periodic: Radio,
    map: [u8; 5],
    interval: u32,
    periodic_anchor: u32,
    next_payload: Option<u64>,
}

impl Discovery {
    pub const fn new() -> Self {
        let radio = Radio {
            address: 0x8e89_bed6,
            crc: 0x555555,
            channel: 37,
        };
        Self {
            stage: Stage::Primary,
            radio,
            broadcast_id: 0,
            sample_rate: 0,
            frame_us: 0,
            frame_bytes: 0,
            bis_counter: 0,
            periodic_counter: 0,
            expected: None,
            adi: 0,
            periodic: radio,
            map: [0; 5],
            interval: 0,
            periodic_anchor: 0,
            next_payload: None,
        }
    }

    pub fn receive(&mut self, raw: &[u8], address_us: u32) -> Result<(), &'static str> {
        if raw.len() < 2 || raw.len() != usize::from(raw[1]) + 2 {
            return Err("PDU length");
        }
        if let Some((start, width)) = self.expected {
            let delta = address_us.wrapping_sub(start) as i32;
            if delta < -20 || i64::from(delta) > i64::from(width) + 20 {
                return Err("anchor timing");
            }
        }
        match self.stage {
            Stage::Primary => {
                let (header, _) = extended(raw)?;
                // ponytail: fixed field subset; add generic extended-header walking for more advertisers.
                if header.len() != 6 || header[0] != 0x18 {
                    return Err("primary fields");
                }
                self.adi = le(&header[1..3]) as u16;
                let aux = &header[3..6];
                let channel = aux[0] & 0x3f;
                let encoded = le(&aux[1..]);
                if channel > 36 || encoded >> 13 != 0 {
                    return Err("AuxPtr PHY/channel");
                }
                let unit = if aux[0] & 0x80 != 0 { 300 } else { 30 };
                let offset = encoded * unit;
                if offset < (raw.len() as u32 + 8) * 8 + 300 {
                    return Err("AuxPtr offset");
                }
                self.expected = Some((address_us.wrapping_add(offset), unit));
                self.radio.channel = channel;
                self.stage = Stage::Auxiliary;
            }
            Stage::Auxiliary => {
                let (header, data) = extended(raw)?;
                if header.len() != 27
                    || header[0] != 0x29
                    || le(&header[7..9]) != u32::from(self.adi)
                {
                    return Err("auxiliary fields/ADI");
                }
                let sync = &header[9..];
                let offset = le(&sync[..2]);
                if offset & 0xc000 != 0 || offset & 0x1fff == 0 {
                    return Err("SyncInfo offset");
                }
                let unit = if offset & 0x2000 != 0 { 300 } else { 30 };
                self.interval = le(&sync[2..4]) * 1250;
                if self.interval < 7500 {
                    return Err("periodic interval");
                }
                self.map.copy_from_slice(&sync[4..9]);
                self.map[4] &= 0x1f;
                self.periodic_counter = le(&sync[16..18]) as u16;
                self.periodic = Radio {
                    address: le(&sync[9..13]),
                    crc: le(&sync[13..16]),
                    channel: 0,
                };
                validate_access_address(self.periodic.address, false)
                    .map_err(|_| "periodic AA")?;
                self.periodic.channel =
                    channel(self.periodic.address, self.periodic_counter, self.map)?;
                let announcement = ad(data, 0x16)?;
                if announcement.len() != 5 || announcement[..2] != [0x52, 0x18] {
                    return Err("broadcast announcement");
                }
                self.broadcast_id = le(&announcement[2..]);
                self.radio = self.periodic;
                self.expected = Some((address_us.wrapping_add((offset & 0x1fff) * unit), unit));
                self.stage = Stage::Periodic;
            }
            Stage::Periodic => {
                let (header, data) = extended(raw)?;
                if header.first() != Some(&0) {
                    return Err("periodic fields");
                }
                let big = ad(&header[1..], 0x2c)?;
                if big.len() != 33 || big[12] != 0 || big[27] >> 5 != 0 || big[32] & 0x80 != 0 {
                    return Err("BIGInfo format/PHY/framing");
                }
                let first = le(&big[..4]);
                let second = le(&big[4..8]);
                let iso_interval = ((first >> 15) & 0xfff) * 1250;
                if first >> 27 != 1
                    || big[4] != 0x21
                    || second >> 28 != 0
                    || big[10] >> 4 != 1
                    || iso_interval != self.interval
                {
                    return Err("BIGInfo scheduling subset");
                }
                let payload = u64::from(le(&big[28..32])) | (u64::from(big[32]) << 32);
                if self.next_payload.is_some_and(|next| next != payload) {
                    return Err("payload count");
                }
                self.next_payload = Some((payload + 1) & ((1 << 39) - 1));
                self.bis_counter = payload as u16;
                let identity = BisParameters::derive(le(&big[13..17]), le(&big[21..23]) as u16, 1)
                    .map_err(|_| "BIS identity")?;
                let mut map = [0; 5];
                map.copy_from_slice(&big[23..28]);
                let base = ad(data, 0x16)?;
                self.parse_base(base)?;
                let sdu = le(&big[17..21]);
                if sdu & 0xfffff != self.frame_us
                    || sdu >> 20 != u32::from(self.frame_bytes)
                    || u16::from(big[11]) != self.frame_bytes
                {
                    return Err("BASE/BIGInfo SDU mismatch");
                }
                let unit = if first & (1 << 14) != 0 { 300 } else { 30 };
                let offset = (first & 0x3fff) * unit;
                if offset < (raw.len() as u32 + 8) * 8
                    || offset + (u32::from(big[11]) + 10) * 8 >= self.interval
                {
                    return Err("BIG offset");
                }
                self.periodic_anchor = address_us;
                self.expected = Some((address_us.wrapping_add(offset), unit));
                self.radio = Radio {
                    address: identity.access_address,
                    crc: identity.crc_init,
                    channel: channel(identity.access_address, self.bis_counter, map)?,
                };
                self.stage = Stage::Bis;
            }
            Stage::Bis => {
                if raw[0] != 0 || u16::from(raw[1]) != self.frame_bytes {
                    return Err("BIS header/SDU");
                }
                self.periodic_counter = self.periodic_counter.wrapping_add(1);
                self.periodic.channel =
                    channel(self.periodic.address, self.periodic_counter, self.map)?;
                self.radio = self.periodic;
                self.expected = Some((self.periodic_anchor.wrapping_add(self.interval), 0));
                self.stage = Stage::Periodic;
            }
        }
        Ok(())
    }

    fn parse_base(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < 13 || data[..2] != [0x51, 0x18] || data[5..12] != [1, 1, 6, 0, 0, 0, 0] {
            return Err("BASE subgroup/codec");
        }
        let config_end = 13 + usize::from(data[12]);
        let config = data.get(13..config_end).ok_or("BASE config length")?;
        let mut rest = config;
        let mut seen = 0u8;
        while let Some((&size, bytes)) = rest.split_first() {
            let item = bytes.get(..usize::from(size)).ok_or("LC3 LTV length")?;
            let kind = *item.first().ok_or("LC3 LTV empty")?;
            if !(1..=5).contains(&kind) || seen & (1 << kind) != 0 {
                return Err("LC3 type/duplicate");
            }
            seen |= 1 << kind;
            match (kind, &item[1..]) {
                (1, [frequency]) => {
                    self.sample_rate = match frequency {
                        1 => 8000,
                        3 => 16000,
                        5 => 24000,
                        6 => 32000,
                        8 => 48000,
                        _ => return Err("LC3 rate"),
                    }
                }
                (2, [duration]) => {
                    self.frame_us = match duration {
                        0 => 7500,
                        1 => 10000,
                        _ => return Err("LC3 duration"),
                    }
                }
                (3, value)
                    if value.len() == 4
                        && le(value).count_ones() <= 1
                        && le(value) & 0xf000_0000 == 0 => {}
                (4, [lo, hi]) => self.frame_bytes = u16::from_le_bytes([*lo, *hi]),
                (5, [1]) => (),
                _ => return Err("LC3 value/subset"),
            }
            rest = &bytes[usize::from(size)..];
        }
        if seen & 0x16 != 0x16 || !(20..=251).contains(&self.frame_bytes) {
            return Err("LC3 mandatory fields");
        }
        let metadata_len = usize::from(*data.get(config_end).ok_or("BASE metadata")?);
        let metadata_end = config_end + 1 + metadata_len;
        let mut metadata = data
            .get(config_end + 1..metadata_end)
            .ok_or("BASE metadata length")?;
        while let Some((&size, bytes)) = metadata.split_first() {
            if size == 0 || bytes.len() < usize::from(size) {
                return Err("BASE metadata LTV");
            }
            metadata = &bytes[usize::from(size)..];
        }
        if data.get(metadata_end..) != Some(&[1, 0][..]) {
            return Err("BASE BIS index/override");
        }
        Ok(())
    }
}

fn le(bytes: &[u8]) -> u32 {
    bytes.iter().enumerate().fold(0, |value, (index, byte)| {
        value | (u32::from(*byte) << (index * 8))
    })
}

fn channel(address: u32, counter: u16, map: [u8; 5]) -> Result<u8, &'static str> {
    IsoChannels::new(address, counter, map)
        .map_err(|_| "channel map")?
        .next()
        .ok_or("channel")
}

fn extended(raw: &[u8]) -> Result<(&[u8], &[u8]), &'static str> {
    if raw.len() < 4 || raw[0] & 0xbf != 7 || raw[2] & 0xc0 != 0 {
        return Err("extended header");
    }
    let end = 3 + usize::from(raw[2] & 0x3f);
    Ok((
        raw.get(3..end).ok_or("extended length")?,
        raw.get(end..).ok_or("advertising data")?,
    ))
}

fn ad(mut bytes: &[u8], kind: u8) -> Result<&[u8], &'static str> {
    let mut found = None;
    while let Some((&size, rest)) = bytes.split_first() {
        let item = rest.get(..usize::from(size)).ok_or("AD length")?;
        if item.first() == Some(&kind) {
            if found.is_some() {
                return Err("duplicate AD");
            }
            found = Some(&item[1..]);
        } else if item.is_empty() {
            return Err("empty AD");
        }
        bytes = &rest[usize::from(size)..];
    }
    found.ok_or("missing AD")
}
