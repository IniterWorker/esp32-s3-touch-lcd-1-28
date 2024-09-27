use std::sync;
use std::time::Instant;

use cst816s::command::Touch;
use cst816s::command::TouchEvent;
use embedded_graphics::prelude::Point;
use esp_idf_hal::delay::Delay;
use esp_idf_hal::gpio::{self, OutputPin, PinDriver};

use lvgl::input_device::{
    pointer::{Pointer, PointerInputData},
    InputDriver,
};
use lvgl::style::Style;
use lvgl::widgets::Arc;
use lvgl::{Align, Color, Display, DrawBuffer, Part, Widget};

use embedded_graphics::draw_target::DrawTarget;

use esp_idf_hal::spi::{
    self,
    config::{Config, Mode, Phase, Polarity},
    SpiDeviceDriver,
};

use esp_idf_hal::prelude::*;

use gc9a01::{prelude::*, Gc9a01, SPIDisplayInterface};

use crate::gyroscope_task::Orientation;

pub struct ThreadDisplayData<'a> {
    #[allow(dead_code)]
    pub shared_orientation: sync::Arc<sync::Mutex<Orientation>>,
    pub shared_cursor: sync::Arc<sync::Mutex<Option<TouchEvent>>>,
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

    // Initialize lvgl
    lvgl::init();

    let mut display_driver = Box::new(Gc9a01::new(
        interface,
        DisplayResolution240x240,
        DisplayRotation::Rotate180,
    ))
    .into_buffered_graphics();
    display_driver
        .reset(&mut reset_output, &mut data.delay)
        .ok();
    display_driver.init(&mut data.delay).ok();
    backlight_output.set_high().unwrap();

    display_driver.clear();

    let buffer = DrawBuffer::<{ 12 * 240_usize }>::default();
    let display = Display::register(buffer, 240, 240, |refresh| {
        display_driver.draw_iter(refresh.as_pixels()).unwrap();
    })
    .unwrap();

    // Create screen and widgets
    let mut screen = display.get_scr_act().unwrap();

    let mut screen_style = Style::default();
    screen_style.set_bg_color(Color::from_rgb((255, 255, 255)));
    screen_style.set_radius(0);

    screen.add_style(Part::Main, &mut screen_style);

    // Create the gauge
    let mut arc_style = Style::default();
    // Set a background color and a radius
    arc_style.set_bg_color(Color::from_rgb((192, 192, 192)));

    // Create the arc object
    let mut arc = Arc::create(&mut screen).unwrap();
    arc.add_style(Part::Main, &mut arc_style);
    arc.set_size(200, 200);
    arc.set_align(Align::Center, 0, 0);
    arc.set_start_angle(135).unwrap();
    arc.set_end_angle(135).unwrap();

    // The read_touchscreen_cb is used by Lvgl to detect touchscreen presses and releases
    let read_touchscreen_cb = || {
        let cursor = data.shared_cursor.try_lock();

        if let Ok(mut cursor) = cursor {
            if let Some(event) = *cursor {
                let ret = match event.touch_type {
                    Touch::Up => PointerInputData::Touch(Point::new(0, 0)).released().once(),
                    Touch::Down | Touch::Contact => {
                        PointerInputData::Touch(Point::new(event.x as i32, event.y as i32))
                            .pressed()
                            .once()
                    }
                };

                *cursor = None;

                ret
            } else {
                PointerInputData::Touch(Point::new(0, 0)).released().once()
            }
        } else {
            PointerInputData::Touch(Point::new(0, 0)).released().once()
        }
    };

    // Register a new input device that's capable of reading the current state of the input
    let _touch_screen = Pointer::register(read_touchscreen_cb, &display).unwrap();

    loop {
        let start = Instant::now();

        lvgl::task_handler();
        data.delay.delay_ms(20);
        display_driver.flush().ok();

        lvgl::tick_inc(Instant::now().duration_since(start));
    }
}
