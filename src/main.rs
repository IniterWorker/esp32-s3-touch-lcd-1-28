use std::sync::{Arc, Mutex};

use cst816s::command::TouchEvent;
use esp_idf_hal::cpu::Core;
use esp_idf_hal::delay::Delay;
use esp_idf_hal::gpio::{self};
use esp_idf_hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_hal::{i2c, spi};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop, hal::peripherals::Peripherals, timer::EspTaskTimerService,
};
use touch_task::{touch_task, TouchTaskData};

use crate::gyroscope_task::{gyroscope_task, Orientation, SensorsTaskData};
use crate::screen_task::{thread_display, ThreadDisplayData};

mod gyroscope_task;
mod screen_task;
mod touch_task;

fn app_main() -> anyhow::Result<()> {
    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;
    let _sysloop = EspSystemEventLoop::take()?;
    let _timer_service = EspTaskTimerService::new()?;

    let i2c_sda = pins.gpio6;
    let i2c_scl = pins.gpio7;

    let sck = pins.gpio10;
    let mosi = pins.gpio11;
    let cs = pins.gpio9;
    let dc = pins.gpio8;
    let reset = pins.gpio14;
    let backlight = pins.gpio2;

    let qmi8658_int1 = pins.gpio4;
    let qmi8658_int2 = pins.gpio3;
    let cst816s_int1 = pins.gpio5;
    let cst816s_reset = pins.gpio13;

    let i2c = i2c::I2cDriver::new(peripherals.i2c0, i2c_sda, i2c_scl, &i2c::I2cConfig::new())?;

    let bus: &'static shared_bus::BusManager<Mutex<i2c::I2cDriver<'_>>> =
        shared_bus::new_std!(i2c::I2cDriver = i2c).unwrap();

    let driver: spi::SpiDriver<'_> = spi::SpiDriver::new(
        peripherals.spi2,
        sck,
        mosi,
        None::<gpio::AnyIOPin>,
        &spi::SpiDriverConfig::new(),
    )?;

    log::info!("Driver configured!");

    // Shared entities
    let shared_orientation: Arc<Mutex<Orientation>> = Arc::new(Mutex::new(Orientation::default()));
    let shared_cursor: Arc<Mutex<Option<TouchEvent>>> = Arc::new(Mutex::new(None));

    // Display task
    ThreadSpawnConfiguration {
        name: Some(b"display\0"),
        stack_size: 7000 + (240 * 240 * 2) + (240 * 12), // only the Builder::new().stack_size is real
        priority: 15,
        pin_to_core: Some(Core::Core1), // Dedicates the `Core::Core1` to display
        ..Default::default()
    }
    .set()?;

    // The first 7000 bytes are hosting the common stack
    // The next 240*240*2 bytes are hosting the buffer from the driver
    // The next 240*12 bytes are hosting the 12 row in the lgvl buffer
    let shared_orientation_cpy = shared_orientation.clone();
    let shared_cursor_cpy = shared_cursor.clone();
    let _thread_1 = std::thread::Builder::new()
        .stack_size(7000 + (240 * 240 * 2) + (240 * 12))
        .spawn(move || {
            thread_display(ThreadDisplayData {
                shared_orientation: shared_orientation_cpy,
                shared_cursor: shared_cursor_cpy,
                backlight: backlight.into(),
                cs: cs.into(),
                dc: dc.into(),
                reset: reset.into(),
                driver,
                delay: Delay::new_default(),
            })
        })?;

    // Gyroscope task
    ThreadSpawnConfiguration {
        name: Some(b"gyroscope\0"),
        stack_size: 7000, // only the Builder::new().stack_size is real
        priority: 10,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;

    let shared_orientation_cpy = shared_orientation.clone();
    let _thread_2 = std::thread::Builder::new()
        .stack_size(7000)
        .spawn(move || {
            gyroscope_task(SensorsTaskData {
                shared_orientation: shared_orientation_cpy,
                bus,
                delay: Delay::new_default(),
                _int1: qmi8658_int1.into(),
                int2: qmi8658_int2.into(),
            })
        })?;

    // Touch screen task
    ThreadSpawnConfiguration {
        name: Some(b"touch\0"),
        stack_size: 7000, // only the Builder::new().stack_size is real
        priority: 10,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;
    let shared_cursor_cpy = shared_cursor.clone();
    let _thread_3 = std::thread::Builder::new()
        .stack_size(7000)
        .spawn(move || {
            touch_task(TouchTaskData {
                shared_cursor: shared_cursor_cpy,
                bus,
                delay: Delay::new_default(),
                int1: cst816s_int1.into(),
                reset: cst816s_reset.into(),
            })
        })?;

    ThreadSpawnConfiguration::default().set()?;

    let _ = _thread_1.join().unwrap();
    let _ = _thread_2.join().unwrap();
    let _ = _thread_3.join().unwrap();
    Ok(())
}

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    match app_main() {
        Ok(()) => log::info!("terminated"),
        Err(e) => log::error!("{:?}", e),
    }
}
