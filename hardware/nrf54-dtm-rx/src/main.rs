#![no_std]
#![no_main]

use core::{
    cell::UnsafeCell,
    sync::atomic::{Ordering, compiler_fence},
};
use embassy_nrf::{config, pac};
use pac::radio::vals::{Crcstatus, Endian, Len, Mode, Plen, Skipaddr};
use {defmt_rtt as _, panic_probe as _};

#[cfg(feature = "discovery")]
mod discovery;
#[cfg(all(feature = "hopping", not(feature = "discovery")))]
use cyw43_controller_proto::channel as iso_channel;

const BIS: bool = cfg!(feature = "bis");
const ACCESS_ADDRESS: u32 = if cfg!(feature = "discovery") {
    0x8e89_bed6
} else if BIS {
    0x718e_bed6
} else {
    0x7176_4129
};
const CRC_INIT: u32 = if BIS && !cfg!(feature = "discovery") {
    0x0012_3401
} else {
    0x0055_5555
};
const FREQUENCY: u8 = if BIS && !cfg!(feature = "discovery") {
    4
} else {
    2
};
const CRC_POLYNOMIAL: u32 = 0x065b;
const PACKET_BYTES: usize = 2 + 255;

#[repr(align(4))]
struct DmaPacket(UnsafeCell<[u8; PACKET_BYTES]>);

#[cfg(all(feature = "hopping", not(feature = "discovery")))]
fn hopping_channel(counter: u16) -> u8 {
    iso_channel::IsoChannels::new(ACCESS_ADDRESS, counter, [255, 255, 255, 255, 31])
        .expect("fixed full channel map")
        .next()
        .expect("first ISO subevent")
}

#[cortex_m_rt::entry]
fn main() -> ! {
    defmt::println!("DTM_RX boot nrf54l15-app-s");
    let mut config = config::Config::default();
    config.hfclk_source = config::HfclkSource::ExternalXtal;
    let _peripherals = embassy_nrf::init(config);
    // NCS direct_test_mode clock_init uses CONSTLAT for nRF54L erratum 20.
    // Keeping constant latency enabled is harmless for this always-on test receiver.
    pac::POWER.tasks_constlat().write_value(1);

    let mut core = cortex_m::Peripherals::take().expect("core peripherals");
    core.DCB.enable_trace();
    core.DWT.enable_cycle_counter();

    #[cfg(feature = "discovery")]
    let mut discovery = discovery::Discovery::new();
    #[cfg(all(feature = "hopping", not(feature = "discovery")))]
    let mut event_counter = 0u16;
    #[cfg(all(feature = "hopping", not(feature = "discovery")))]
    let mut channel = hopping_channel(event_counter);
    #[cfg(feature = "discovery")]
    let channel = 37u8;
    #[cfg(not(any(feature = "hopping", feature = "discovery")))]
    let channel = 0u8;
    let frequency = if cfg!(all(feature = "hopping", not(feature = "discovery"))) {
        2 * channel + if channel <= 10 { 4 } else { 6 }
    } else {
        FREQUENCY
    };
    let radio = pac::RADIO;
    for interrupt in 0..2 {
        radio.intenclr0(interrupt).write(|w| w.0 = u32::MAX);
        radio.intenclr1(interrupt).write(|w| w.0 = u32::MAX);
    }
    radio.shorts().write(|w| {
        w.set_ready_start(true);
        w.set_phyend_disable(true);
    });
    // Same BLE 1M packet recipe as NCS direct_test_mode/src/dtm.c radio_init.
    radio.mode().write(|w| w.set_mode(Mode::Ble1mbit));
    radio.frequency().write(|w| {
        w.set_frequency(frequency);
        w.set_map(false);
    });
    radio.base0().write_value(ACCESS_ADDRESS << 8);
    radio.prefix0().write(|w| w.0 = ACCESS_ADDRESS >> 24);
    radio.rxaddresses().write(|w| w.0 = 1);
    radio.txaddress().write(|w| w.0 = 0);
    radio.crccnf().write(|w| {
        w.set_len(Len::Three);
        w.set_skipaddr(Skipaddr::Skip);
    });
    radio.crcinit().write(|w| w.0 = CRC_INIT);
    radio.crcpoly().write(|w| w.0 = CRC_POLYNOMIAL);
    radio.pcnf0().write(|w| {
        w.set_s0len(true);
        w.set_lflen(8);
        w.set_s1len(0);
        w.set_plen(Plen::_8bit);
    });
    radio.pcnf1().write(|w| {
        w.set_maxlen(255);
        w.set_statlen(0);
        w.set_balen(3);
        w.set_endian(Endian::Little);
        w.set_whiteen(BIS);
    });
    // nRF54L needs the full BLE polynomial and bit 6 seed (Zephyr radio_nrf54lx.h).
    radio.datawhite().write(|w| {
        w.set_poly(0x89);
        w.set_iv(0x40 | u16::from(channel));
    });

    #[cfg(feature = "timestamps")]
    {
        let timer = pac::TIMER10;
        timer.tasks_stop().write_value(1);
        timer.intenclr().write(|w| w.0 = u32::MAX);
        timer.shorts().write(|w| w.0 = 0);
        timer
            .mode()
            .write(|w| w.set_mode(pac::timer::vals::Mode::Timer));
        timer
            .bitmode()
            .write(|w| w.set_bitmode(pac::timer::vals::Bitmode::_32bit));
        // TIMER10's 32 MHz clock / 2^5 gives 1 us; RADIO shares DPPI domain 10.
        timer.prescaler().write(|w| w.set_prescaler(5));
        for (index, publish) in [radio.publish_address(), radio.publish_phyend()]
            .into_iter()
            .enumerate()
        {
            publish.write(|w| {
                w.set_chidx(index as u8);
                w.set_en(true);
            });
            timer.subscribe_capture(index).write(|w| {
                w.set_chidx(index as u8);
                w.set_en(true);
            });
        }
        pac::DPPIC10.chenset().write(|w| w.0 = 3);
        timer.tasks_clear().write_value(1);
        timer.tasks_start().write_value(1);
        defmt::println!(
            "DTM_RX timestamps timer=TIMER10 hz=1000000 address_cc=0 phyend_cc=1 dppi=0,1"
        );
    }

    let packet = DmaPacket(UnsafeCell::new([0; PACKET_BYTES]));
    let pointer = packet.0.get().cast::<u8>();
    radio.packetptr().write_value(pointer as u32);
    let mut received = 0u32;
    let mut good = 0u32;
    let mut heartbeat = cortex_m::peripheral::DWT::cycle_count();
    #[cfg(feature = "timestamps")]
    let mut previous_address: Option<u32> = None;
    defmt::println!(
        "DTM_RX ready mhz={} mode=BLE1M aa=0x{:08x} crc_init=0x{:06x} whitening={} channel={}",
        2400 + u16::from(frequency),
        ACCESS_ADDRESS,
        CRC_INIT,
        BIS,
        channel
    );

    loop {
        radio.events_ready().write_value(0);
        radio.events_address().write_value(0);
        radio.events_phyend().write_value(0);
        radio.events_disabled().write_value(0);
        radio.events_crcok().write_value(0);
        radio.events_crcerror().write_value(0);
        compiler_fence(Ordering::SeqCst);
        cortex_m::asm::dmb();
        radio.tasks_rxen().write_value(1);
        while radio.events_disabled().read() == 0 {
            let now = cortex_m::peripheral::DWT::cycle_count();
            if now.wrapping_sub(heartbeat) >= 128_000_000 {
                heartbeat = now;
                defmt::println!(
                    "DTM_RX waiting total={} good={} state={} ready={} address={}",
                    received,
                    good,
                    radio.state().read().0,
                    radio.events_ready().read(),
                    radio.events_address().read()
                );
            }
        }
        cortex_m::asm::dmb();
        compiler_fence(Ordering::SeqCst);
        let crc_ok = radio.crcstatus().read().crcstatus() == Crcstatus::CrcOk;
        let crc = radio.rxcrc().read().rxcrc();
        let mut snapshot = [0u8; PACKET_BYTES];
        // SAFETY: packet is aligned live RAM, exclusively owned by this function.
        // PHYEND_DISABLE has stopped RADIO DMA before any CPU access. Volatile
        // reads observe hardware writes; no RXEN occurs until the copy is done.
        unsafe {
            for (index, byte) in snapshot.iter_mut().enumerate() {
                *byte = pointer.add(index).read_volatile();
            }
        }
        let length = usize::from(snapshot[1]) + 2;
        #[cfg(feature = "discovery")]
        if discovery.stage == discovery::Stage::Primary
            && (!crc_ok
                // Select this fixture's SID/DID; radio continuation still comes from received fields.
                || snapshot.get(4..6) != Some(&[0xbc, 0x3a][..])
                || radio.events_address().read() == 0
                || radio.events_phyend().read() == 0
                || discovery::Discovery::new()
                    .receive(&snapshot[..length], pac::TIMER10.cc(0).read())
                    .is_err())
        {
            continue;
        }
        received = received.wrapping_add(1);
        good = good.wrapping_add(u32::from(crc_ok));
        #[cfg(feature = "timestamps")]
        {
            // PHYEND_DISABLE stops RX before these hardware captures are read.
            let address = pac::TIMER10.cc(0).read();
            let phyend = pac::TIMER10.cc(1).read();
            let valid = radio.events_address().read() != 0 && radio.events_phyend().read() != 0;
            let first = previous_address.is_none();
            let delta = previous_address.map_or(0, |previous| address.wrapping_sub(previous));
            defmt::println!(
                "DTM_RX timing packet={} valid={} first={} address_us={} phyend_us={} delta_us={}",
                received,
                valid,
                first,
                address,
                phyend,
                delta
            );
            #[cfg(all(feature = "hopping", not(feature = "discovery")))]
            if !valid || !crc_ok || (!first && delta.abs_diff(10_000) > 20) {
                defmt::println!(
                    "DTM_RX hopping_desync packet={} counter={} channel={} crc_ok={} delta_us={}",
                    received,
                    event_counter,
                    channel,
                    crc_ok,
                    delta
                );
                // ponytail: stop on loss; timed channel advancement needs an acquired BIG anchor.
                loop {
                    cortex_m::asm::wfi();
                }
            }
            previous_address = valid.then_some(address);
        }
        // ponytail: receiver pauses while logging; use double-buffer DMA for lossless capture.
        defmt::println!(
            "DTM_RX packet={} crc_ok={} rx_crc=0x{:06x} raw={=[u8]:02x}",
            received,
            crc_ok,
            crc,
            &snapshot[..length]
        );
        #[cfg(feature = "discovery")]
        {
            let stage = match discovery.stage {
                discovery::Stage::Primary => 0u8,
                discovery::Stage::Auxiliary => 1,
                discovery::Stage::Periodic => 2,
                discovery::Stage::Bis => 3,
            };
            let current = discovery.radio;
            let valid = radio.events_address().read() != 0 && radio.events_phyend().read() != 0;
            let result = if valid && crc_ok {
                discovery.receive(&snapshot[..length], pac::TIMER10.cc(0).read())
            } else {
                Err("CRC/capture")
            };
            if let Err(error) = result {
                defmt::println!(
                    "DTM_RX discovery_error packet={} stage={} error={}",
                    received,
                    stage,
                    error
                );
                loop {
                    cortex_m::asm::wfi();
                }
            }
            defmt::println!(
                "DTM_RX discovery packet={} stage={} channel={} aa=0x{:08x} next_channel={} next_aa=0x{:08x} broadcast_id=0x{:06x} rate={} frame_us={} frame_bytes={} periodic_counter={} bis_counter={}",
                received,
                stage,
                current.channel,
                current.address,
                discovery.radio.channel,
                discovery.radio.address,
                discovery.broadcast_id,
                discovery.sample_rate,
                discovery.frame_us,
                discovery.frame_bytes,
                discovery.periodic_counter,
                discovery.bis_counter
            );
            let next = discovery.radio;
            radio.base0().write_value(next.address << 8);
            radio.prefix0().write(|w| w.0 = next.address >> 24);
            radio.crcinit().write(|w| w.0 = next.crc);
            radio.frequency().write(|w| {
                w.set_frequency(2 * next.channel + if next.channel <= 10 { 4 } else { 6 })
            });
            radio
                .datawhite()
                .modify(|w| w.set_iv(0x40 | u16::from(next.channel)));
        }
        #[cfg(all(feature = "hopping", not(feature = "discovery")))]
        {
            let next_counter = event_counter.wrapping_add(1);
            let next_channel = hopping_channel(next_counter);
            defmt::println!(
                "DTM_RX hop packet={} counter={} channel={} next_counter={} next_channel={}",
                received,
                event_counter,
                channel,
                next_counter,
                next_channel
            );
            event_counter = next_counter;
            channel = next_channel;
            radio.frequency().write(|w| {
                w.set_frequency(2 * channel + if channel <= 10 { 4 } else { 6 });
            });
            radio
                .datawhite()
                .modify(|w| w.set_iv(0x40 | u16::from(channel)));
        }
    }
}
