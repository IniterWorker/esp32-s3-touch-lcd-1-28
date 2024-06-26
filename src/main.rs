use esp_idf_hal::delay::{Delay, FreeRtos};
use esp_idf_hal::gpio::{self, OutputPin, PinDriver};
use esp_idf_hal::spi::{
    self,
    config::{Config, Mode, Phase, Polarity},
    SpiDeviceDriver,
};
use esp_idf_hal::{i2c, prelude::*};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop, hal::peripherals::Peripherals, timer::EspTaskTimerService,
};

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::{Point, RgbColor, Size},
    primitives::{Circle, Primitive, PrimitiveStyleBuilder, Rectangle},
    Drawable,
};
use gc9a01::{mode::BufferedGraphics, prelude::*, Gc9a01, SPIDisplayInterface};
use qmi8658::Qmi8658;

type BoxedDisplayDriver<'a> = Box<
    Gc9a01<
        SPIInterface<
            SpiDeviceDriver<'a, spi::SpiDriver<'a>>,
            PinDriver<'a, gpio::AnyOutputPin, gpio::Output>,
        >,
        DisplayResolution240x240,
        BufferedGraphics<DisplayResolution240x240>,
    >,
>;

/// Test Function : will be removed later
fn draw<I: WriteOnlyDataCommand, D: DisplayDefinition>(
    display: &mut Gc9a01<I, D, BufferedGraphics<D>>,
    tick: u32,
) {
    let (w, h) = display.dimensions();
    let w = w as u32;
    let h = h as u32;
    let x = tick % w;
    let y = tick % h;

    let style = PrimitiveStyleBuilder::new()
        .stroke_width(4)
        .stroke_color(Rgb565::new(tick as u8, x as u8, y as u8))
        .fill_color(Rgb565::RED)
        .build();

    let cdiameter = 20;

    // circle
    Circle::new(
        Point::new(119 - cdiameter / 2 + 40, 119 - cdiameter / 2 + 40),
        cdiameter as u32,
    )
    .into_styled(style)
    .draw(display)
    .unwrap();

    // circle
    Circle::new(
        Point::new(119 - cdiameter / 2 - 40, 119 - cdiameter / 2 + 40),
        cdiameter as u32,
    )
    .into_styled(style)
    .draw(display)
    .unwrap();

    // rectangle
    let rw = 80;
    let rh = 20;
    Rectangle::new(
        Point::new(119 - rw / 2, 119 - rh / 2 - 40),
        Size::new(rw as u32, rh as u32),
    )
    .into_styled(style)
    .draw(display)
    .unwrap();
}

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();
    let pins = peripherals.pins;
    let _sysloop = EspSystemEventLoop::take().unwrap();
    let _timer_service = EspTaskTimerService::new().unwrap();
    let mut delay = Delay::new_default();

    let sck = pins.gpio10;
    let mosi = pins.gpio11;
    let cs = pins.gpio9;
    let dc = pins.gpio8;
    let reset = pins.gpio14;
    let backlight = pins.gpio2;
    let i2c_sda = pins.gpio6;
    let i2c_scl = pins.gpio7;
    let _qmi8658_int1 = pins.gpio4;
    let _qmi8658_int2 = pins.gpio3;

    let cs_output = cs;
    let dc_output = PinDriver::output(dc.downgrade_output()).unwrap();
    let mut backlight_output = PinDriver::output(backlight.downgrade_output()).unwrap();
    let mut reset_output = PinDriver::output(reset.downgrade_output()).unwrap();

    backlight_output.set_high().unwrap();

    let i2c =
        i2c::I2cDriver::new(peripherals.i2c0, i2c_sda, i2c_scl, &i2c::I2cConfig::new()).unwrap();
    let mut qmi8658_device = Qmi8658::new_secondary_address(i2c, delay);
    log::info!("Driver configured!");

    match qmi8658_device.get_device_id() {
        Ok(value) => log::info!("QMI8658 Found: {:?}", value),
        Err(e) => log::error!("QMI8658 Error: {:?}", e),
    }

    let driver = spi::SpiDriver::new(
        peripherals.spi2,
        sck,
        mosi,
        None::<gpio::AnyIOPin>,
        &spi::SpiDriverConfig::new(),
    )
    .unwrap();

    let config = Config::new().baudrate(2.MHz().into()).data_mode(Mode {
        polarity: Polarity::IdleLow,
        phase: Phase::CaptureOnFirstTransition,
    });

    let spi_device = SpiDeviceDriver::new(driver, Some(cs_output), &config).unwrap();

    let interface = SPIDisplayInterface::new(spi_device, dc_output);

    let mut display_driver: BoxedDisplayDriver = Box::new(
        Gc9a01::new(
            interface,
            DisplayResolution240x240,
            DisplayRotation::Rotate0,
        )
        .into_buffered_graphics(),
    );

    display_driver.reset(&mut reset_output, &mut delay).ok();
    display_driver.init(&mut delay).ok();
    log::info!("Driver configured!");

    let mut tick: u32 = 0;
    loop {
        display_driver.clear();
        draw(&mut display_driver, tick);
        display_driver.flush().ok();
        tick += 1;
        FreeRtos::delay_ms(2000);
    }
}
