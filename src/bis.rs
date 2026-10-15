//! BIS/BIG Control Access Address and CRC initialization derivation.
//!
//! Bluetooth Core 6.0, Vol 6, Part B, sections 2.1.2 and 3.1.1:
//! <https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-60/out/en/low-energy-controller/link-layer-specification.html>
//! Cross-checked with Zephyr v4.2.0 `util_bis_aa_le32` and `lll_adv_iso.c`:
//! <https://github.com/zephyrproject-rtos/zephyr/blob/v4.2.0/subsys/bluetooth/controller/util/util.c#L281>
//! <https://github.com/zephyrproject-rtos/zephyr/blob/v4.2.0/subsys/bluetooth/controller/ll_sw/nordic/lll/lll_adv_iso.c#L228>
//! This does not transmit, advertise `BIGInfo`, choose channels, or generate CRCs.

use core::fmt;

/// Derived radio identity for one BIS, or BIG Control when BIS number is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BisParameters {
    /// 32-bit Access Address; use `to_le_bytes()` for packet-order octets.
    pub access_address: u32,
    /// 24-bit CRC shift-register initial value, not an encoded packet CRC.
    pub crc_init: u32,
}

impl BisParameters {
    /// Derives one BIS identity from `BIGInfo`'s seed and 16-bit `BaseCRCInit`.
    ///
    /// `bis_number` is 1..=31 for a BIS and zero for BIG Control. Seed generation
    /// must use physical entropy; this deterministic function provides none.
    /// Caller must check same-device address uniqueness, ensure different BIG
    /// seeds' low 16 bits differ by at least two bits, and validate all active
    /// BIS addresses plus BIG Control. If the implementation supports LE Coded,
    /// also apply [`validate_access_address`] with `coded_supported = true` to
    /// the seed and each derived address.
    ///
    /// # Errors
    /// Returns invalid BIS number, seed constraints, or derived address pattern.
    pub fn derive(seed: u32, base_crc_init: u16, bis_number: u8) -> Result<Self, BisError> {
        if bis_number > 31 {
            return Err(BisError::BisNumber);
        }
        let bit15 = (seed >> 15) & 1;
        let bit16 = (seed >> 16) & 1;
        if (seed >> 19) & 1 != bit15
            || (seed >> 22) & 1 != bit16
            || bit15 == bit16
            || seed & (1 << 25) != 0
            || seed & (1 << 23) == 0
            || validate_access_address(seed, false).is_err()
        {
            return Err(BisError::Seed);
        }
        let diversifier = (35 * u32::from(bis_number) + 42) % 128;
        // DW[31:26] repeats D[0]; other set positions follow Core 2.1.2.
        let word = if diversifier & 1 == 0 { 0 } else { 0xfc00_0000 }
            | ((diversifier & 2) << 24)
            | ((diversifier & 64) << 18)
            | ((diversifier & 2) << 22)
            | ((diversifier & 0x30) << 16)
            | ((diversifier & 0x0c) << 15);
        let access_address = seed ^ word;
        validate_access_address(access_address, false)?;
        Ok(Self {
            access_address,
            crc_init: (u32::from(base_crc_init) << 8) | u32::from(bis_number),
        })
    }

    /// CSA #2 channel identifier: XOR of the address's two 16-bit halves.
    #[must_use]
    pub fn channel_id(self) -> u16 {
        let bytes = self.access_address.to_le_bytes();
        u16::from_le_bytes([bytes[0] ^ bytes[2], bytes[1] ^ bytes[3]])
    }
}

/// Checks Access Address bit-pattern rules; device-wide uniqueness is external.
///
/// `coded_supported` describes the implementation, not only the selected PHY:
/// Core applies the extra rules when the implementation supports LE Coded.
///
/// # Errors
/// Returns an invalid general or LE Coded address pattern.
pub fn validate_access_address(address: u32, coded_supported: bool) -> Result<(), BisError> {
    let transitions = (address ^ (address >> 1)) & 0x7fff_ffff;
    let octets = address.to_le_bytes();
    if (address ^ 0x8e89_bed6).count_ones() <= 1
        || octets.iter().all(|octet| *octet == octets[0])
        || transitions.count_ones() > 24
        || (transitions >> 26).count_ones() < 2
    {
        return Err(BisError::AccessAddress);
    }
    let mut run = 1;
    for bit in 1..32 {
        if ((address >> bit) ^ (address >> (bit - 1))) & 1 == 0 {
            run += 1;
            if run > 6 {
                return Err(BisError::AccessAddress);
            }
        } else {
            run = 1;
        }
    }
    if coded_supported
        && ((address & 0xff).count_ones() < 3 || (transitions & 0x7fff).count_ones() > 11)
    {
        return Err(BisError::CodedAccessAddress);
    }
    Ok(())
}

/// Invalid BIG seed, stream index, or Access Address pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BisError {
    BisNumber,
    Seed,
    AccessAddress,
    CodedAccessAddress,
}

impl fmt::Display for BisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BIS identity error: {self:?}")
    }
}

impl core::error::Error for BisError {}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixed fixture only: real BIGs require entropy and same-device checks.
    const SEED: u32 = 0x8c88_bed6;

    #[test]
    fn core_bis_one_diversifier_and_crc_octet_order() {
        let params = BisParameters::derive(SEED, 0x1234, 1).unwrap();
        assert_eq!(params.access_address ^ SEED, 0xfd06_0000);
        assert_eq!(
            params.access_address.to_le_bytes(),
            [0xd6, 0xbe, 0x8e, 0x71]
        );
        assert_eq!(params.crc_init, 0x12_3401);
        assert_eq!(&params.crc_init.to_le_bytes()[..3], &[1, 0x34, 0x12]);
        assert_eq!(params.channel_id(), 0xcf58);
        assert_eq!(
            BisParameters::derive(SEED, 0xffff, 0).unwrap().crc_init,
            0xff_ff00
        );
    }

    #[test]
    fn every_bis_and_control_match_zephyr_reference() {
        // Compiled Zephyr v4.2.0 util/util.c util_bis_aa_le32 with SEED.
        // First vector is BIG Control; remaining vectors are BIS numbers 1..31.
        let expected = [
            0x8e2c_bed6,
            0x718e_bed6,
            0x8db8_bed6,
            0x7218_bed6,
            0x8e3a_bed6,
            0x719c_bed6,
            0x8dbe_bed6,
            0x721e_bed6,
            0x8f08_bed6,
            0x71aa_bed6,
            0x8c8c_bed6,
            0x722c_bed6,
            0x8f0e_bed6,
            0x71b8_bed6,
            0x8c9a_bed6,
            0x723a_bed6,
            0x8f1c_bed6,
            0x71be_bed6,
            0x8ca8_bed6,
            0x7308_bed6,
            0x8f2a_bed6,
            0x708c_bed6,
            0x8cae_bed6,
            0x730e_bed6,
            0x8f38_bed6,
            0x709a_bed6,
            0x8cbc_bed6,
            0x731c_bed6,
            0x8f3e_bed6,
            0x70a8_bed6,
            0x8d8a_bed6,
            0x732a_bed6,
        ];
        for (index, address) in expected.into_iter().enumerate() {
            let params = BisParameters::derive(SEED, 0x1234, u8::try_from(index).unwrap()).unwrap();
            assert_eq!(params.access_address, address);
            validate_access_address(address, true).unwrap();
        }
    }

    #[test]
    fn invalid_number_seed_and_address_patterns_are_rejected() {
        assert_eq!(BisParameters::derive(SEED, 0, 32), Err(BisError::BisNumber));
        for bit in [15, 16, 19, 22, 23, 25] {
            assert_eq!(
                BisParameters::derive(SEED ^ (1 << bit), 0, 1),
                Err(BisError::Seed)
            );
        }
        for address in [
            0,
            u32::MAX,
            0x8e89_bed6,
            0x8e89_bed7,
            0xaaaa_aaaa,
            0x1212_1212,
        ] {
            assert_eq!(
                validate_access_address(address, false),
                Err(BisError::AccessAddress)
            );
        }
    }

    #[test]
    fn coded_extra_rules_are_checked_only_when_supported() {
        for address in [0x8c88_be11, 0x8c88_aaaa] {
            validate_access_address(address, false).unwrap();
            assert_eq!(
                validate_access_address(address, true),
                Err(BisError::CodedAccessAddress)
            );
        }
    }
}
