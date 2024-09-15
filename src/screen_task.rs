use std::sync::{Arc, Mutex};

use cst816s::command::KeyEvent;
use embedded_fps::{StdClock, FPS};
use esp_idf_hal::delay::Delay;
use esp_idf_hal::gpio::{self, OutputPin, PinDriver};

use esp_idf_hal::spi::{
    self,
    config::{Config, Mode, Phase, Polarity},
    SpiDeviceDriver,
};

use esp_idf_hal::prelude::*;

use gc9a01::{mode::BufferedGraphics, prelude::*, Gc9a01, SPIDisplayInterface};

use crate::gyroscope_task::Orientation;
use crate::screen_draw::{DrawContext, DrawEngine};

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

pub struct ThreadDisplayData<'a> {
    pub shared_orientation: Arc<Mutex<Orientation>>,
    pub shared_cursor: Arc<Mutex<KeyEvent>>,
    pub backlight: gpio::AnyOutputPin,
    pub cs: gpio::AnyOutputPin,
    pub dc: gpio::AnyOutputPin,
    pub reset: gpio::AnyOutputPin,
    pub driver: spi::SpiDriver<'a>,
    pub delay: Delay,
}

pub fn thread_display(mut data: ThreadDisplayData) -> anyhow::Result<()> {
    let dc_output = PinDriver::output(data.dc.downgrade_output())?;
    let mut backlight_output = PinDriver::output(data.backlight.downgrade_output())?;
    let mut reset_output = PinDriver::output(data.reset.downgrade_output())?;

    let config = Config::new().baudrate(40.MHz().into()).data_mode(Mode {
        polarity: Polarity::IdleLow,
        phase: Phase::CaptureOnFirstTransition,
    });
    let spi_device = SpiDeviceDriver::new(data.driver, Some(data.cs), &config)?;
    let interface = SPIDisplayInterface::new(spi_device, dc_output);
    backlight_output.set_low()?;
    let mut display_driver: BoxedDisplayDriver = Box::new(
        Gc9a01::new(
            interface,
            DisplayResolution240x240,
            DisplayRotation::Rotate180,
        )
        .into_buffered_graphics(),
    );
    display_driver
        .reset(&mut reset_output, &mut data.delay)
        .ok();
    display_driver.init(&mut data.delay).ok();
    backlight_output.set_high()?;

    let mut engine = DrawEngine::new();
    let mut engine_context = DrawContext::default();
    let mut fps_counter = FPS::<100, _>::new(StdClock::default());

    loop {
        let orientation = data.shared_orientation.try_lock();

        if let Ok(orientation) = orientation {
            engine_context.counter_lock += 1;
            engine_context.orientation = *orientation;
        }

        let cursor = data.shared_cursor.try_lock();

        if let Ok(cursor) = cursor {
            engine_context.cursor = *cursor;
        }
        engine_context.counter += 1;

        if let Ok(_fps) = fps_counter.try_tick() {
            display_driver.clear();
            engine.draw(&mut display_driver, &engine_context);
            display_driver.flush().ok();
        }
    }
}
