//! Channel Selection Algorithm #2 for CIS/BIS events and subevents.
//!
//! Bluetooth Core Vol 6, Part B, section 4.5.8.3; sample data in Part C section 3.
//! Equations/vectors cross-checked against Zephyr v4.2.0 `lll_chan.c`:
//! <https://github.com/zephyrproject-rtos/zephyr/blob/v4.2.0/subsys/bluetooth/controller/ll_sw/lll_chan.c>
//! This selects data-channel indices only. The caller supplies the appropriate
//! event counter/access address, applies channel-map instants, and drives radio.

use core::fmt;

/// A data-channel map must enable at least two of channels 0..=36, with RFU zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidChannelMap;

impl fmt::Display for InvalidChannelMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("channel map must enable at least two channels and clear RFU bits")
    }
}

impl core::error::Error for InvalidChannelMap {}

/// Allocation-free sequence for one ISO event. First item is first subevent.
/// Consume exactly NSE items; create a fresh iterator at each event boundary.
#[derive(Debug)]
pub struct IsoChannels {
    channels: [u8; 37],
    count: u8,
    channel_id: u16,
    random: u16,
    remap_index: u8,
    first: bool,
}

impl IsoChannels {
    /// Selects an event's channels using the low 16 bits of its ISO event counter.
    /// `access_address` is the stream's 32-bit little-endian wire value.
    /// CIS/BIS access-address validity and negotiated NSE are checked upstream.
    ///
    /// # Errors
    /// Rejects maps with fewer than two enabled channels or nonzero RFU bits.
    pub fn new(
        access_address: u32,
        event_counter: u16,
        channel_map: [u8; 5],
    ) -> Result<Self, InvalidChannelMap> {
        if channel_map[4] & 0xe0 != 0 {
            return Err(InvalidChannelMap);
        }
        let mut channels = [0; 37];
        let mut count = 0_u8;
        for channel in 0..37_u8 {
            if channel_map[usize::from(channel / 8)] & (1 << (channel % 8)) != 0 {
                channels[usize::from(count)] = channel;
                count += 1;
            }
        }
        if count < 2 {
            return Err(InvalidChannelMap);
        }
        let bytes = access_address.to_le_bytes();
        let channel_id =
            u16::from_le_bytes([bytes[0], bytes[1]]) ^ u16::from_le_bytes([bytes[2], bytes[3]]);
        let mut random = event_counter ^ channel_id;
        for _ in 0..3 {
            random = permute_mam(random, channel_id);
        }
        let scrambled = random ^ channel_id;
        let unmapped = scrambled % 37;
        let remap_index = (0..count)
            .find(|&index| u16::from(channels[usize::from(index)]) == unmapped)
            .unwrap_or_else(|| scale(scrambled, count));
        Ok(Self {
            channels,
            count,
            channel_id,
            random,
            remap_index,
            first: true,
        })
    }
}

impl Iterator for IsoChannels {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.first {
            self.first = false;
        } else {
            self.random = permute_mam(self.random, self.channel_id);
            let distance = 1
                .max(3.min(self.count.saturating_sub(5)))
                .max(11.min(self.count.saturating_sub(10) / 2));
            let spread = (self.count + 1).saturating_sub(2 * distance);
            self.remap_index =
                (self.remap_index + distance + scale(self.random ^ self.channel_id, spread))
                    % self.count;
        }
        Some(self.channels[usize::from(self.remap_index)])
    }
}

fn permute_mam(value: u16, channel_id: u16) -> u16 {
    value
        .reverse_bits()
        .swap_bytes()
        .wrapping_mul(17)
        .wrapping_add(channel_id)
}

fn scale(value: u16, count: u8) -> u8 {
    // Multiplication by a fraction strictly below 1 cannot exceed the u8 count.
    #[expect(clippy::cast_possible_truncation)]
    let result = ((u32::from(value) * u32::from(count)) >> 16) as u8;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn core_full_and_sparse_channel_subevent_vectors() {
        // Core sample channel identifier 0x305f, derived from 0x8e89bed6.
        for (counter, expected) in [
            (0, [25, 1, 16, 36]),
            (1, [20, 36, 12, 34]),
            (2, [6, 18, 32, 21]),
            (3, [21, 4, 22, 8]),
        ] {
            let actual: Vec<_> = IsoChannels::new(0x8e89_bed6, counter, [255, 255, 255, 255, 31])
                .unwrap()
                .take(4)
                .collect();
            assert_eq!(actual, expected);
        }
        for (counter, expected) in [
            (6, [23, 35, 21, 36]),
            (7, [9, 22, 36, 33]),
            (8, [34, 9, 33, 10]),
        ] {
            let actual: Vec<_> = IsoChannels::new(0x8e89_bed6, counter, [0, 6, 224, 0, 30])
                .unwrap()
                .take(4)
                .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn two_channel_map_alternates_and_rfu_or_empty_maps_fail() {
        let actual: Vec<_> = IsoChannels::new(0x8e89_bed6, 11, [6, 0, 0, 0, 0])
            .unwrap()
            .take(4)
            .collect();
        assert_eq!(actual, [1, 2, 1, 2]);
        for map in [[0; 5], [1, 0, 0, 0, 0], [3, 0, 0, 0, 0x80]] {
            assert!(matches!(
                IsoChannels::new(0, 0, map),
                Err(InvalidChannelMap)
            ));
        }
    }

    #[test]
    fn every_enabled_channel_count_stays_in_map_without_repeating_neighbors() {
        for count in 2..=37_u8 {
            let mut map = [0; 5];
            for channel in 0..count {
                map[usize::from(channel / 8)] |= 1 << (channel % 8);
            }
            for counter in [0, 1, 32767, 32768, 65535] {
                let sequence: Vec<_> = IsoChannels::new(0x1234_5678, counter, map)
                    .unwrap()
                    .take(31)
                    .collect();
                assert!(sequence.iter().all(|&channel| channel < count));
                assert!(sequence.windows(2).all(|pair| pair[0] != pair[1]));
            }
        }
    }
}
