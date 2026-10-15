//! Fixed laboratory identities and 3750-us links for the controller experiment.
//! Run: `cargo run --example broadcast_fixture -- work`
//! Use `--continuous` after the output directory for 20-ms periodic advertising.

use cyw43_iso_re::{
    broadcast::{BigInfo, BroadcastPhy, SingleBisBase},
    broadcast_advertising::{AdvertisingOffset, AdvertisingSet, SyncInfo},
    le_audio::Lc3Config,
};
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = PathBuf::from(args.next().ok_or("expected output directory")?);
    let continuous = match args.next() {
        None => false,
        Some(option) if option == "--continuous" => true,
        Some(_) => return Err("expected optional --continuous".into()),
    };
    if args.next().is_some() {
        return Err("expected output directory and optional --continuous".into());
    }
    let offset = AdvertisingOffset {
        value: 125,
        units_300_us: false,
    };
    let map = [255, 255, 255, 255, 31];
    let advertiser = AdvertisingSet {
        address: [1, 2, 3, 4, 5, 0xc6],
        random_static: true,
        sid: 3,
        did: 0xabc,
        broadcast_id: 0x12_3456,
    };
    let sync = SyncInfo {
        offset,
        interval: if continuous { 16 } else { 8 },
        channel_map: map,
        sleep_clock_accuracy: 0,
        access_address: 0x1234_5678,
        crc_init: 0xab_cdef,
        event_counter: 0x1234,
    };
    let big = BigInfo {
        offset: 125,
        offset_units_300_us: false,
        iso_interval: 8,
        num_bis: 1,
        nse: 1,
        bn: 1,
        sub_interval_us: 500,
        pto: 0,
        bis_spacing_us: 500,
        irc: 1,
        max_pdu: 26,
        seed_access_address: 0x8c88_bed6,
        sdu_interval_us: 10_000,
        max_sdu: 26,
        base_crc_init: 0x1234,
        channel_map: map,
        phy: BroadcastPhy::Le1M,
        payload_count: 0,
        framed: false,
    };
    let base = SingleBisBase {
        presentation_delay_us: 40_000,
        config: Lc3Config::parse(&[2, 1, 1, 2, 2, 1, 3, 4, 26, 0])?,
        streaming_contexts: 4,
    };
    fs::create_dir_all(&directory)?;
    for (name, pdu) in [
        ("primary", advertiser.primary_pdu(17, offset, true)?),
        ("auxiliary", advertiser.auxiliary_pdu(&sync)?),
        ("periodic", sync.periodic_pdu(&big, &base)?),
    ] {
        fs::write(directory.join(format!("discovery-{name}.bin")), &pdu)?;
        println!("{name}: {} PDU bytes", pdu.len());
    }
    Ok(())
}
