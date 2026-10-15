#![no_std]
#![no_main]

use core::sync::atomic::{Ordering, compiler_fence};
use embassy_nrf::{config, pac};
use pac::radio::vals::{Endian, Len, Mode, Plen, Skipaddr};
use {defmt_rtt as _, panic_probe as _};

#[repr(align(4))]
struct Packet([u8; 36]);

fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}

#[cortex_m_rt::entry]
fn main() -> ! {
    let mut config = config::Config::default();
    config.hfclk_source = config::HfclkSource::ExternalXtal;
    let _peripherals = embassy_nrf::init(config);
    // Same nRF54L erratum 20 workaround as the existing DTM receiver.
    pac::POWER.tasks_constlat().write_value(1);
    let Some(mut core) = cortex_m::Peripherals::take() else {
        halt();
    };
    core.DCB.enable_trace();
    core.DWT.enable_cycle_counter();

    let mut packet = Packet([0; 36]);
    let length = if cfg!(feature = "marker-iso") { 26 } else { 31 };
    packet.0[1] = length;
    let offset = if cfg!(feature = "marker-alt") {
        0x80
    } else {
        0
    };
    for (index, byte) in packet.0[2..2 + usize::from(length)].iter_mut().enumerate() {
        *byte = offset + index as u8;
    }
    if cfg!(feature = "marker-counter") {
        packet.0[2..4].copy_from_slice(&0u16.to_le_bytes());
    }
    let (frequency, access_address, crc_init, whitening) = if cfg!(feature = "marker-bis") {
        (4, 0x718e_bed6u32, 0x123401, true)
    } else {
        (2, 0x7176_4129u32, 0x555555, false)
    };
    let crc_init = crc_init ^ u32::from(cfg!(feature = "marker-bad-crc"));
    let radio = pac::RADIO;
    for index in 0..2 {
        radio.intenclr0(index).write(|w| w.0 = u32::MAX);
        radio.intenclr1(index).write(|w| w.0 = u32::MAX);
    }
    radio.shorts().write(|w| {
        w.set_ready_start(true);
        w.set_phyend_disable(true);
    });
    radio.mode().write(|w| w.set_mode(Mode::Ble1mbit));
    radio.frequency().write(|w| {
        w.set_frequency(frequency);
        w.set_map(false);
    });
    radio.base0().write_value(access_address << 8);
    radio.prefix0().write(|w| w.0 = access_address >> 24);
    radio.txaddress().write(|w| w.0 = 0);
    radio.crccnf().write(|w| {
        w.set_len(Len::Three);
        w.set_skipaddr(Skipaddr::Skip);
    });
    radio.crcinit().write(|w| w.0 = crc_init);
    radio.crcpoly().write(|w| w.0 = 0x065b);
    radio.pcnf0().write(|w| {
        w.set_s0len(true);
        w.set_lflen(8);
        w.set_s1len(0);
        w.set_plen(Plen::_8bit);
    });
    radio.pcnf1().write(|w| {
        w.set_maxlen(length);
        w.set_statlen(0);
        w.set_balen(3);
        w.set_endian(Endian::Little);
        w.set_whiteen(whitening);
    });
    radio.datawhite().write(|w| {
        w.set_poly(0x89);
        w.set_iv(0x40);
    });
    // Packet stays live; counter changes only between completed transmissions.
    radio.packetptr().write_value(packet.0.as_ptr() as u32);
    compiler_fence(Ordering::SeqCst);
    cortex_m::asm::dmb();
    defmt::println!(
        "DTM_TX ready mhz={} mode=BLE1M aa={:08x} crc_init={:06x} whitening={} header={:04x} payload={=[u8]:02x}",
        2400 + u16::from(frequency),
        access_address,
        crc_init,
        whitening,
        u16::from(length) << 8,
        packet.0[2..2 + usize::from(length)]
    );
    if cfg!(feature = "marker-counter") {
        defmt::println!("DTM_TX counter first=0 count=500 payload-offset=0 width=2 endian=little");
    }

    // ponytail: bounded marker source only; add precise scheduling for cadence tests.
    for sent in 1u16..=500 {
        if cfg!(feature = "marker-counter") {
            packet.0[2..4].copy_from_slice(&(sent - 1).to_le_bytes());
        }
        compiler_fence(Ordering::SeqCst);
        cortex_m::asm::dmb();
        let start = cortex_m::peripheral::DWT::cycle_count();
        radio.events_disabled().write_value(0);
        radio.tasks_txen().write_value(1);
        while radio.events_disabled().read() == 0 {
            if cortex_m::peripheral::DWT::cycle_count().wrapping_sub(start) > 128_000_000 {
                radio.tasks_disable().write_value(1);
                defmt::println!("DTM_TX failed timeout sent={}", sent - 1);
                halt();
            }
        }
        // CPUAPP is 128 MHz; spacing is approximate and not a timing claim.
        while cortex_m::peripheral::DWT::cycle_count().wrapping_sub(start) < 1_280_000 {
            core::hint::spin_loop();
        }
    }
    core::hint::black_box(&packet);
    defmt::println!("DTM_TX done sent=500 disabled=true");
    halt()
}
