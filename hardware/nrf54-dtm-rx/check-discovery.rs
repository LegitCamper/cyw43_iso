//! Run: rustc --edition 2024 --test check-discovery.rs -o /tmp/check-discovery && /tmp/check-discovery
extern crate alloc;
#[path = "../../src/bis.rs"]
mod bis;
#[path = "src/discovery.rs"]
mod discovery;
#[path = "../../src/iso_channel.rs"]
mod iso_channel;

#[cfg(test)]
fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|s| u8::from_str_radix(s, 16).unwrap())
        .collect()
}
#[cfg(test)]
fn fixtures() -> [Vec<u8>; 3] {
    [
        bytes("07 07 06 18 bc 3a 51 7d 00"),
        bytes(
            "47 23 1b 29 01 02 03 04 05 c6 bc 3a 7d 00 08 00 ff ff ff ff 1f 78 56 34 12 ef cd ab 34 12 06 16 52 18 56 34 12",
        ),
        bytes(
            "07 45 24 00 22 2c 7d 00 04 08 21 f4 01 00 f4 01 10 1a 00 d6 be 88 8c 10 27 a0 01 34 12 ff ff ff ff 1f 00 00 00 00 00 1f 16 51 18 40 9c 00 01 01 06 00 00 00 00 0a 02 01 01 02 02 01 03 04 1a 00 04 03 02 04 00 01 00",
        ),
    ]
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: check-discovery CAPTURE.txt");
    let input = std::fs::read_to_string(path).expect("read capture");
    let mut timings = std::collections::BTreeMap::new();
    let mut rx = discovery::Discovery::new();
    let mut counts = [0; 4];
    let mut next = 1u32;
    for line in input.lines() {
        assert!(
            !line.contains("discovery_error"),
            "receiver reported failure: {line}"
        );
        let number = |key: &str| -> u32 {
            line.split_once(key)
                .expect("missing log key")
                .1
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        if line.contains("DTM_RX timing packet=") {
            assert!(line.contains("valid=true"), "invalid captures");
            timings.insert(
                number("packet="),
                (number("address_us="), number("phyend_us=")),
            );
        } else if line.contains("DTM_RX packet=") {
            let packet = number("packet=");
            assert_eq!(packet, next, "missing packets");
            next += 1;
            assert!(line.contains("crc_ok=true"), "bad CRC");
            let hex = line
                .split_once("raw=[")
                .unwrap()
                .1
                .split_once(']')
                .unwrap()
                .0;
            let raw: Vec<_> = hex
                .split(',')
                .map(|byte| u8::from_str_radix(byte.trim(), 16).unwrap())
                .collect();
            let (address, end) = timings.remove(&packet).expect("missing timestamp");
            assert!(
                end.wrapping_sub(address)
                    .abs_diff((raw.len() as u32 + 3) * 8)
                    <= 3,
                "packet airtime"
            );
            counts[rx.stage as usize] += 1;
            rx.receive(&raw, address)
                .unwrap_or_else(|error| panic!("packet {packet}: {error}"));
        }
    }
    assert_eq!(&counts[..2], &[1, 1]);
    assert!(counts[3] >= 8, "need at least eight BIS packets");
    assert!(
        (counts[2]..=counts[2] + 1).contains(&(counts[3] + 1)),
        "periodic/BIS sequence"
    );
    println!(
        "Discovery capture passes: {} periodic, {} BIS; broadcast_id={:#08x}, LC3 {} Hz / {} us / {} bytes",
        counts[2], counts[3], rx.broadcast_id, rx.sample_rate, rx.frame_us, rx.frame_bytes
    );
}

#[test]
fn discovery_uses_announcements_and_tracks_wrapping_timestamps() {
    let packets = fixtures();
    let mut rx = discovery::Discovery::new();
    let anchor = u32::MAX - 2000;
    for (i, packet) in packets.iter().enumerate() {
        rx.receive(packet, anchor.wrapping_add(i as u32 * 3750))
            .unwrap();
    }
    assert_eq!(rx.radio.address, 0x718ebed6);
    assert_eq!(rx.radio.crc, 0x123401);
    assert_eq!(rx.radio.channel, 1);
    assert_eq!(
        (rx.sample_rate, rx.frame_us, rx.frame_bytes),
        (8000, 10000, 26)
    );
    assert_eq!(rx.broadcast_id, 0x123456);
    let mut sdu = [0; 28];
    sdu[1] = 26;
    rx.receive(&sdu, anchor.wrapping_add(11250)).unwrap();
    assert_eq!(rx.periodic_counter, 0x1235);
    let mut periodic = packets[2].clone();
    periodic[34] = 1;
    rx.receive(&periodic, anchor.wrapping_add(17500)).unwrap();
    assert_eq!(rx.radio.channel, 28);
    assert_eq!(rx.bis_counter, 1);
}

#[test]
fn truncation_and_bad_anchors_rejected_without_panics() {
    let packets = fixtures();
    for stage in 0..3 {
        for length in 0..packets[stage].len() {
            let mut rx = discovery::Discovery::new();
            for (i, packet) in packets.iter().enumerate().take(stage) {
                rx.receive(packet, i as u32 * 3750).unwrap();
            }
            assert!(
                rx.receive(&packets[stage][..length], stage as u32 * 3750)
                    .is_err()
            );
        }
    }
    let mut rx = discovery::Discovery::new();
    rx.receive(&packets[0], 0).unwrap();
    assert!(rx.receive(&packets[1], 5000).is_err());
}

#[test]
fn changed_aux_channel_and_periodic_identity_are_parsed() {
    let mut packets = fixtures();
    packets[0][6] = 0x40 | 3;
    packets[1][12 + 9..12 + 13].copy_from_slice(&0x8765_4321u32.to_le_bytes());
    let mut rx = discovery::Discovery::new();
    rx.receive(&packets[0], 0).unwrap();
    assert_eq!(rx.radio.channel, 3);
    rx.receive(&packets[1], 3750).unwrap();
    assert_eq!(rx.radio.address, 0x87654321);
}
