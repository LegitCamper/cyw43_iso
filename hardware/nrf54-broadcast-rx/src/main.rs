#![no_std]
#![no_main]

mod discovery;

use bt_hci::{
    ControllerToHostPacket,
    cmd::{
        AsyncCmd, SyncCmd,
        controller_baseband::{Reset, SetEventMask},
        le::*,
    },
    controller::Controller,
    data::IsoPacketBoundary,
    event::{Event, EventKind, le::LeEvent},
    param::*,
};
use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_nrf::{bind_interrupts, config, cracen, mode::Blocking};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

async fn session(
    controller: &sdc::SoftdeviceController<'_>,
) -> Result<(), bt_hci::cmd::Error<sdc::Error>> {
    defmt::info!("NRF54_BROADCAST_RX command=Reset opcode=0x0c03");
    Reset::new().exec(controller).await?;
    defmt::info!("NRF54_BROADCAST_RX command=SetEventMask opcode=0x0c01");
    SetEventMask::new(
        EventMask::new()
            .enable_le_meta(true)
            .enable_hardware_error(true),
    )
    .exec(controller)
    .await?;
    defmt::info!("NRF54_BROADCAST_RX command=LeSetEventMask opcode=0x2001");
    LeSetEventMask::new(
        LeEventMask::new()
            .enable_le_ext_adv_report(true)
            .enable_le_periodic_adv_sync_established_v1(true)
            .enable_le_periodic_adv_report_v1(true)
            .enable_le_periodic_adv_sync_lost(true)
            .enable_le_biginfo_adv_report(true)
            .enable_le_big_sync_established(true)
            .enable_le_big_sync_lost(true),
    )
    .exec(controller)
    .await?;
    defmt::info!("NRF54_BROADCAST_RX command=LeSetRandomAddr opcode=0x2005");
    // ponytail: lab scanner identity; derive a device-unique address for deployment.
    LeSetRandomAddr::new(BdAddr::new([0x61, 0x62, 0x63, 0x64, 0x65, 0xc0]))
        .exec(controller)
        .await?;
    defmt::info!("NRF54_BROADCAST_RX command=LeSetExtScanParams opcode=0x2041");
    LeSetExtScanParams::new(
        AddrKind::RANDOM,
        ScanningFilterPolicy::BasicUnfiltered,
        PhyParams {
            le_1m_phy: Some(ScanningPhy {
                active_scan: false,
                scan_interval: Duration::from_u16(160),
                scan_window: Duration::from_u16(160),
            }),
            le_2m_phy: None,
            le_coded_phy: None,
        },
    )
    .exec(controller)
    .await?;
    defmt::info!("NRF54_BROADCAST_RX command=LeSetExtScanEnable opcode=0x2042");
    LeSetExtScanEnable::new(
        true,
        FilterDuplicates::Disabled,
        Duration::from_u16(0),
        Duration::from_u16(0),
    )
    .exec(controller)
    .await?;
    defmt::info!("NRF54_BROADCAST_RX scanning");
    let mut buffer = unwrap!(controller.alloc_buf());
    let mut periodic_pending = false;
    if cfg!(feature = "preconfigured") {
        defmt::info!("NRF54_BROADCAST_RX command=LePeriodicAdvCreateSync opcode=0x2044");
        LePeriodicAdvCreateSync::new(
            LePeriodicAdvCreateSyncOptions::new(),
            3,
            AddrKind::RANDOM,
            BdAddr::new([1, 2, 3, 4, 5, 0xc6]),
            0,
            Duration::from_u16(0x4000),
            CteMask::new(),
        )
        .exec(controller)
        .await?;
        periodic_pending = true;
        defmt::info!("NRF54_BROADCAST_RX preconfigured-identity-sync-armed");
    }
    let mut periodic_handle = None;
    let mut base_seen = false;
    let mut big_info_seen = false;
    let mut big_pending = false;
    let mut bis_handle = None;
    let mut iso_count = 0_u32;
    loop {
        let packet = match controller.read(&mut buffer).await {
            Ok(packet) => packet,
            Err(error) => {
                defmt::error!("NRF54_BROADCAST_RX read-error={:?}", error);
                return Ok(());
            }
        };
        match packet {
            ControllerToHostPacket::Event(packet) => {
                if packet.kind == EventKind::Le
                    && packet.data.len() >= 2
                    && matches!(packet.data[0], 0x0e | 0x1d)
                    && packet.data[1] != 0
                {
                    // Failed sync fields need not contain valid typed PHY values.
                    defmt::warn!("NRF54_BROADCAST_RX sync-failed raw={:02x}", packet.data);
                    return Ok(());
                }
                let event = match Event::try_from(packet.clone()) {
                    Ok(event) => event,
                    Err(_) => {
                        defmt::warn!(
                            "NRF54_BROADCAST_RX malformed-event kind={} raw={:02x}",
                            packet.kind.0,
                            packet.data
                        );
                        continue;
                    }
                };
                match event {
                    Event::Le(LeEvent::LeExtendedAdvertisingReport(event)) if !periodic_pending => {
                        for report in event.reports.iter().flatten() {
                            if report.adv_sid != 3
                                || report.addr_kind != AddrKind::RANDOM
                                || report.addr.raw() != [1, 2, 3, 4, 5, 0xc6]
                                || !matches!(
                                    report.event_kind.data_status(),
                                    LeExtAdvDataStatus::Complete
                                )
                                || discovery::service_data(report.data, 0x1852)
                                    != Some(&[0x56, 0x34, 0x12][..])
                            {
                                continue;
                            }
                            defmt::info!(
                                "NRF54_BROADCAST_RX discovered rssi={} data={:02x}",
                                report.rssi,
                                report.data
                            );
                            LePeriodicAdvCreateSync::new(
                                LePeriodicAdvCreateSyncOptions::new(),
                                report.adv_sid,
                                report.addr_kind,
                                report.addr,
                                0,
                                Duration::from_u16(0x4000),
                                CteMask::new(),
                            )
                            .exec(controller)
                            .await?;
                            periodic_pending = true;
                            break;
                        }
                    }
                    Event::Le(LeEvent::LePeriodicAdvertisingSyncEstablished(event)) => {
                        defmt::info!("NRF54_BROADCAST_RX periodic-established {:?}", event);
                        periodic_handle = Some(event.sync_handle);
                        LeSetExtScanEnable::new(
                            false,
                            FilterDuplicates::Disabled,
                            Duration::from_u16(0),
                            Duration::from_u16(0),
                        )
                        .exec(controller)
                        .await?;
                    }
                    Event::Le(LeEvent::LePeriodicAdvertisingReport(event))
                        if Some(event.sync_handle) == periodic_handle =>
                    {
                        if !base_seen {
                            defmt::info!(
                                "NRF54_BROADCAST_RX periodic-data status={:?} data={:02x}",
                                event.data_status,
                                event.data
                            );
                        }
                        if event.data_status == DataStatus::Complete
                            && discovery::matches_base(event.data)
                        {
                            base_seen = true;
                        }
                    }
                    Event::Le(LeEvent::LeBiginfoAdvertisingReport(event))
                        if Some(event.sync_handle) == periodic_handle =>
                    {
                        if !big_info_seen {
                            defmt::info!("NRF54_BROADCAST_RX biginfo {:?}", event);
                        }
                        big_info_seen = event.num_bis == 1
                            && event.nse == 1
                            && event.bn == 1
                            && event.irc == 1
                            && event.iso_interval == 8
                            && event.max_pdu == 26
                            && event.max_sdu == 26
                            && event.sdu_interval == ExtDuration::from_u32(10000)
                            && event.phy == PhyKind::Le1M
                            && !event.is_framed
                            && !event.is_encrypted;
                    }
                    Event::Le(LeEvent::LeBigSyncEstablished(event)) => {
                        defmt::info!("NRF54_BROADCAST_RX big-established {:?}", event);
                        let Some(handle) =
                            event.bis_handles.first().and_then(|bis| bis.handle().ok())
                        else {
                            return Ok(());
                        };
                        LeSetupIsoDataPath::new(
                            handle,
                            DataPathDirection::Output,
                            DataPathId::HCI,
                            CodecId {
                                coding_format: 3,
                                company_id: 0,
                                vendor_specific_codec_id: 0,
                            },
                            ExtDuration::from_u32(0),
                            &[],
                        )
                        .exec(controller)
                        .await?;
                        bis_handle = Some(handle);
                        defmt::info!("NRF54_BROADCAST_RX iso-ready handle={}", handle.raw());
                    }
                    Event::Le(LeEvent::LePeriodicAdvertisingSyncLost(event)) => {
                        defmt::warn!("NRF54_BROADCAST_RX periodic-lost {:?}", event);
                        return Ok(());
                    }
                    Event::Le(LeEvent::LeBigSyncLost(event)) => {
                        defmt::warn!("NRF54_BROADCAST_RX big-lost {:?}", event);
                        return Ok(());
                    }
                    Event::HardwareError(event) => {
                        defmt::error!("NRF54_BROADCAST_RX hardware-error {:?}", event);
                        return Ok(());
                    }
                    _ => {}
                }
            }
            ControllerToHostPacket::Iso(packet) if Some(packet.handle()) == bis_handle => {
                iso_count = iso_count.wrapping_add(1);
                if let Some(header) = packet.data_load_header() {
                    let status = header.iso_sdu_len >> 14;
                    let length = header.iso_sdu_len & 0x0fff;
                    let valid = packet.boundary_flag() == IsoPacketBoundary::Complete
                        && status == 0
                        && length == 26
                        && packet.data().len() == 26;
                    defmt::info!(
                        "NRF54_BROADCAST_RX iso count={} seq={} status={} len={} valid={} data={:02x}",
                        iso_count,
                        header.sequence_num,
                        status,
                        length,
                        valid,
                        packet.data()
                    );
                } else {
                    defmt::warn!(
                        "NRF54_BROADCAST_RX iso-fragment count={} data={:02x}",
                        iso_count,
                        packet.data()
                    );
                }
            }
            _ => {}
        }
        if base_seen
            && big_info_seen
            && !big_pending
            && let Some(handle) = periodic_handle
        {
            LeBigCreateSync::new(
                BigHandle(0),
                handle,
                EncryptionMode::Unencrypted,
                BroadcastCode::new([0; 16]),
                0,
                0x4000,
                &[1],
            )
            .exec(controller)
            .await?;
            big_pending = true;
            defmt::info!("NRF54_BROADCAST_RX big-sync-pending");
        }
    }
}

bind_interrupts!(struct Irqs {
    SWI00 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO_0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER10 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    GRTC_3 => nrf_sdc::mpsl::HighPrioInterruptHandler;
});

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

fn build_sdc<'d, const N: usize>(
    p: sdc::Peripherals<'d>,
    rng: &'d mut cracen::Cracen<'static, Blocking>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<sdc::SoftdeviceController<'d>, sdc::Error> {
    sdc::Builder::new()?
        .support_scan()
        .support_ext_scan()
        .support_le_periodic_sync()
        .support_bis_sink()
        .big_count(1)?
        .bis_sink_count(1)?
        .periodic_sync_count(1)?
        .periodic_sync_buffer_cfg(2)?
        .scan_buffer_cfg(8)?
        .iso_buffer_cfg(0, 0, 0, 6, 8, 128)?
        .build(p, rng, mpsl, mem)
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::info!("start");
    let mut config: config::Config = Default::default();
    config.clock_speed = config::ClockSpeed::CK128;
    config.hfclk_source = config::HfclkSource::ExternalXtal;
    config.lfclk_source = config::LfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);
    defmt::info!("clocks initialized");
    let mpsl_p = mpsl::Peripherals::new(
        p.GRTC_CH7,
        p.GRTC_CH8,
        p.GRTC_CH9,
        p.GRTC_CH10,
        p.GRTC_CH11,
        p.TIMER10,
        p.TIMER20,
        p.TEMP,
        p.PPI10_CH0,
        p.PPI20_CH1,
        p.PPIB11_CH0,
        p.PPIB21_CH0,
    );
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_XTAL as u8,
        rc_ctiv: 0,
        rc_temp_ctiv: 0,
        accuracy_ppm: 50,
        skip_wait_lfclk_started: false,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(
        mpsl_p, Irqs, lfclk_cfg
    )));
    spawner.spawn(unwrap!(mpsl_task(&*mpsl)));

    let sdc_p = sdc::Peripherals::new(
        p.PPI00_CH1,
        p.PPI00_CH3,
        p.PPI10_CH1,
        p.PPI10_CH2,
        p.PPI10_CH3,
        p.PPI10_CH4,
        p.PPI10_CH5,
        p.PPI10_CH6,
        p.PPI10_CH7,
        p.PPI10_CH8,
        p.PPI10_CH9,
        p.PPI10_CH10,
        p.PPI10_CH11,
        p.PPIB00_CH1,
        p.PPIB00_CH2,
        p.PPIB00_CH3,
        p.PPIB10_CH1,
        p.PPIB10_CH2,
        p.PPIB10_CH3,
    );

    let mut rng = cracen::Cracen::new_blocking(p.CRACEN);
    // Bump if `build_sdc` fails - it logs the exact number of bytes needed.
    static SDC_MEM: StaticCell<sdc::Mem<16384>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init_with(sdc::Mem::new);
    let sdc = unwrap!(build_sdc(sdc_p, &mut rng, mpsl, sdc_mem));

    defmt::info!("NRF54_BROADCAST_RX ready stock-SDC target-id=123456 sid=3");
    loop {
        if let Err(error) = session(&sdc).await {
            defmt::error!("NRF54_BROADCAST_RX command-error={:?}", error);
        }
        embassy_time::Timer::after_secs(1).await;
    }
}
