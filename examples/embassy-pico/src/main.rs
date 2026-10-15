#![no_std]
#![no_main]

use bt_hci::{ControllerToHostPacket, FromHciBytes, transport::Transport};
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_rp::{
    bind_interrupts, dma,
    gpio::{Level, Output},
    peripherals::{DMA_CH0, PIO0},
    pio::{InterruptHandler, Pio},
};
use embassy_time::{Duration, Instant, Timer};
use panic_probe as _;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
});

const AUDIO: &[u8; 520] = include_bytes!("../../../work/embassy-runtime/audio.bin");

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    defmt::info!("EMBASSY_ISO begin");
    let p = embassy_rp::init(Default::default());
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        dma::Channel::new(p.DMA_CH0, Irqs),
    );
    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let (_net, driver, _control, mut runner) = cyw43::new_with_bluetooth(
        STATE.init(cyw43::State::new()),
        pwr,
        spi,
        cyw43::aligned_bytes!("../../../work/embassy-runtime/wifi.bin"),
        cyw43::aligned_bytes!("../../../work/sram-exec-probe/zero-append.bin"),
        cyw43::aligned_bytes!("../../../work/embassy-runtime/nvram.bin"),
    )
    .await;
    defmt::info!("EMBASSY_ISO initialized");
    // Keep early startup evidence in RTT long enough for a probe to attach.
    Timer::after_secs(2).await;
    let mut diagnostic = [0; 260];
    for address in [0x203a34, 0x203a80, 0x201f1c] {
        let _ = runner.bluetooth_runtime_read(address, &mut diagnostic[..4]).await;
        if address == 0x201f1c {
            let context = u32::from_le_bytes(diagnostic[..4].try_into().unwrap());
            if context & 3 == 0 && (0x200000..=0x21dd00).contains(&context) {
                let _ = runner.bluetooth_runtime_read(context + 256, &mut diagnostic[..4]).await;
            }
        }
    }

    match runner.bluetooth_runtime_command(&[3, 0x0c, 0], &mut diagnostic).await {
        Ok(length) => defmt::info!("EMBASSY_ISO reset={=[u8]:02x}", diagnostic[..length]),
        Err(error) => { defmt::error!("EMBASSY_ISO reset failed {:?}", defmt::Debug2Format(&error)); halt().await; }
    }
    match runner.bluetooth_runtime_command(&[1, 0x10, 0], &mut diagnostic).await {
        Ok(length) => defmt::info!("EMBASSY_ISO version={=[u8]:02x}", diagnostic[..length]),
        Err(error) => { defmt::error!("EMBASSY_ISO version failed {:?}", defmt::Debug2Format(&error)); halt().await; }
    }
    for (address, length) in [(0x203a34, 4), (0x203a80, 4), (0x201f1c, 4), (0x200560, 12),
        (0x209788, 48), (0xd0f90, 112), (0xd035c, 16), (0x8b634, 44)] {
        let result = runner.bluetooth_runtime_read(address, &mut diagnostic[..length]).await;
        defmt::info!("EMBASSY_ISO memory {=u32:08x} ok={} bytes={=[u8]:02x}", address, result.is_ok(), diagnostic[..length]);
        if address == 0x201f1c && result.is_ok() {
            let context = u32::from_le_bytes(diagnostic[..4].try_into().unwrap());
            if context & 3 == 0 && (0x200000..=0x21dd00).contains(&context) {
                let result = runner.bluetooth_runtime_read(context + 256, &mut diagnostic[..4]).await;
                defmt::info!("EMBASSY_ISO task {=u32:08x} ok={} bytes={=[u8]:02x}", context + 256, result.is_ok(), diagnostic[..4]);
            }
        }
    }
    let mut installed = match cyw43_iso_embassy_example::runtime::install(&mut runner).await {
        Ok(value) => value,
        Err(error) => {
            defmt::error!(
                "EMBASSY_ISO install failed {:?}; reset required",
                defmt::Debug2Format(&error)
            );
            halt().await
        }
    };
    defmt::info!("EMBASSY_ISO allocation={=u32:08x}", installed.allocation);
    let mut sent = 0u16;
    let mut completed = 0u16;
    let mut paused = false;
    let mut pause_until = Instant::now();
    // Prefill four packets before the radio is armed, matching the C experiment.
    for sequence in 0..4 {
        send(&driver, sequence).await;
        assert!(!runner.bluetooth_runtime_poll().await.unwrap());
        sent += 1;
    }
    if let Err(error) = installed.start(&mut runner).await {
        defmt::error!(
            "EMBASSY_ISO start failed {:?}; reset required",
            defmt::Debug2Format(&error)
        );
        halt().await;
    }
    let deadline = Instant::now() + Duration::from_millis(1000);
    let mut input = [0; 1024];
    let mut good = true;
    while Instant::now() < deadline && completed < 20 {
        match runner.bluetooth_runtime_poll().await {
            Ok(true) => match driver.read(&mut input).await {
                Ok(ControllerToHostPacket::Event(event))
                    if event.kind.0 == 0x13 && event.data.len() == 5 && event.data[0] == 1 =>
                {
                    let handle = u16::from_le_bytes([event.data[1], event.data[2]]);
                    let count = u16::from_le_bytes([event.data[3], event.data[4]]);
                    if handle != 0x42 || count == 0 || count > sent - completed {
                        good = false;
                        break;
                    }
                    completed += count;
                    defmt::info!("EMBASSY_ISO credit={} total={}", count, completed);
                }
                _ => {
                    good = false;
                    break;
                }
            },
            Ok(false) => {}
            Err(_) => {
                good = false;
                break;
            }
        }
        if !good {
            break;
        }
        if sent == 8 && !paused {
            paused = true;
            pause_until = Instant::now() + Duration::from_millis(80);
            defmt::info!("EMBASSY_ISO pause=80ms");
        }
        if sent < 20 && sent - completed < 4 && Instant::now() >= pause_until {
            send(&driver, sent).await;
            sent += 1;
        }
        Timer::after_micros(100).await;
    }
    let mut native = [0; 40];
    let mut queue = [0; 32];
    good &= runner
        .bluetooth_runtime_read(installed.allocation + 2056, &mut native)
        .await
        .is_ok();
    good &= runner
        .bluetooth_runtime_read(installed.allocation + 1792, &mut queue)
        .await
        .is_ok();
    let word =
        |bytes: &[u8], i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
    defmt::info!(
        "EMBASSY_ISO sent={} accepted={} consumed={} rf={} reported={} credits={} underruns={} inflight={} blocked={} head={} tail={}",
        sent,
        word(&native, 3),
        word(&queue, 4),
        word(&queue, 6),
        word(&native, 9),
        completed,
        word(&queue, 5),
        word(&queue, 7),
        word(&native, 2),
        word(&queue, 0),
        word(&queue, 1)
    );
    good &= sent == 20
        && completed == 20
        && word(&native, 3) == 20
        && word(&queue, 4) == 20
        && word(&queue, 6) == 20
        && word(&native, 9) == 20
        && word(&queue, 5) > 0
        && word(&queue, 7) == 0
        && word(&native, 2) == 0
        && word(&queue, 0) == word(&queue, 1);
    let restored = installed.restore(&mut runner).await.is_ok();
    defmt::info!(
        "EMBASSY_ISO restored={} pass={}",
        restored,
        good && restored
    );
    halt().await;
}

async fn send(driver: &cyw43::bluetooth::BtDriver<'_>, sequence: u16) {
    let mut wire = [0; 34];
    wire[..8].copy_from_slice(&[0x42, 0x20, 30, 0, sequence as u8, 0, 26, 0]);
    wire[8..].copy_from_slice(&AUDIO[usize::from(sequence) * 26..usize::from(sequence + 1) * 26]);
    let packet = bt_hci::data::IsoPacket::from_hci_bytes_complete(&wire).unwrap();
    driver.write(&packet).await.unwrap();
}

async fn halt() -> ! {
    loop {
        Timer::after_secs(1).await;
    }
}
