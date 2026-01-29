#![no_std]
#![no_main]

use crate::scsi::ScsiHandler;
use crate::{msc::MscHandler, scsi::SdmmcScsi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::USB;
use embassy_time::Timer;
use embassy_usb::handlers::UsbHostHandler;
use embassy_usb::host::UsbHostBusExt;
use embassy_usb_driver::host::DeviceEvent::Connected;
use embassy_usb_driver::host::UsbHostDriver;
use embedded_sdmmc::asynchronous::VolumeManager;

use {defmt_rtt as _, panic_probe as _};

mod msc;
mod scsi;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::host::InterruptHandler<USB>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Create the driver, from the HAL.
    let mut usbhost = embassy_rp::usb::host::Driver::new(p.USB, Irqs);

    debug!("Detecting device");
    // Wait for root-port to detect device
    let speed = loop {
        match usbhost.wait_for_device_event().await {
            Connected(speed) => break speed,
            _ => {}
        }
    };

    debug!("Found device with speed = {:?}", speed);

    let enum_info = usbhost.enumerate_root_bare(speed, 1).await.unwrap();
    let msc = MscHandler::try_register(&usbhost, &enum_info)
        .await
        .expect("Couldn't mass storage device");

    let mut scsi = ScsiHandler::new(msc);
    scsi.init().await;

    let sdmmc_scsi = SdmmcScsi::new(scsi);

    let volume_mgr = VolumeManager::<_, _, MAX_DIRS, MAX_FILES, MAX_VOLUMES>::new_with_limits(
        sdmmc_scsi,
        DummyTimesource,
        5000,
    );

    let mut volume0 = volume_mgr
        .open_volume(embedded_sdmmc::asynchronous::VolumeIdx(0))
        .await
        .unwrap();
    info!("Volume 0: {:?}", defmt::Debug2Format(&volume0));

    let mut root_dir = volume0.open_root_dir().unwrap();

    let mut my_file = root_dir
        .open_file_in_dir("MY_FILE.TXT", embedded_sdmmc::asynchronous::Mode::ReadOnly)
        .await
        .unwrap();

    while !my_file.is_eof() {
        let mut buf = [0u8; 32];
        if let Ok(n) = my_file.read(&mut buf).await {
            info!("{:a}", buf[..n]);
        }
    }

    loop {
        Timer::after_secs(1).await
    }
}

pub const MAX_DIRS: usize = 4;
pub const MAX_FILES: usize = 5;
pub const MAX_VOLUMES: usize = 1;
// Max file or dir name string len
pub const MAX_NAME_LEN: usize = 25;

struct DummyTimesource;

impl embedded_sdmmc::asynchronous::TimeSource for DummyTimesource {
    fn get_timestamp(&self) -> embedded_sdmmc::asynchronous::Timestamp {
        embedded_sdmmc::asynchronous::Timestamp {
            year_since_1970: 0,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}
