pub fn service_data(mut data: &[u8], uuid: u16) -> Option<&[u8]> {
    while let Some((&length, rest)) = data.split_first() {
        let item = rest.get(..usize::from(length))?;
        if length == 0 {
            return None;
        }
        if item.len() >= 3 && item[0] == 0x16 && item[1..3] == uuid.to_le_bytes() {
            return Some(&item[3..]);
        }
        data = &rest[usize::from(length)..];
    }
    None
}

pub fn matches_base(data: &[u8]) -> bool {
    let Some(base) = service_data(data, 0x1851) else {
        return false;
    };
    // ponytail: only generated 8 kHz/10 ms/26-byte mono BASE; add LC3 LTV negotiation for other sources.
    base.get(3..)
        == Some(
            &[
                1, 1, 6, 0, 0, 0, 0, 10, 2, 1, 1, 2, 2, 1, 3, 4, 26, 0, 4, 3, 2, 4, 0, 1, 0,
            ][..],
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcement_boundaries_and_fixture_base() {
        let announce = [2, 1, 6, 6, 0x16, 0x52, 0x18, 0x56, 0x34, 0x12];
        assert_eq!(
            service_data(&announce, 0x1852),
            Some(&[0x56, 0x34, 0x12][..])
        );
        for n in 0..announce.len() {
            assert!(service_data(&announce[..n], 0x1852).is_none());
        }
        assert!(service_data(&[0, 0x16, 0x52, 0x18], 0x1852).is_none());
        let mut base = [
            31, 0x16, 0x51, 0x18, 0x40, 0x9c, 0, 1, 1, 6, 0, 0, 0, 0, 10, 2, 1, 1, 2, 2, 1, 3, 4,
            26, 0, 4, 3, 2, 4, 0, 1, 0,
        ];
        assert!(matches_base(&base));
        for n in 0..base.len() {
            assert!(!matches_base(&base[..n]));
        }
        base[23] = 40;
        assert!(!matches_base(&base));
    }
}
