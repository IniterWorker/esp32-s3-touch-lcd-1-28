//! Blocking reference test for gc9a01-rs 0.5 on the ESP32-S3-Touch-LCD-1.28.
//!
//! Runs the reference sequence against the blocking `Gc9a01` driver and
//! prints `GC9A01-REF mode=blocking ...` markers on UART; `scripts/hil.sh`
//! greps them. The sequence, markers, delays and flush timings are
//! identical to `embassy_async.rs` (only the mode string, `.await`s, delay
//! type and the heartbeat task differ) — keep them that way.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embedded_hal::delay::DelayNs;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Level, Output, OutputConfig},
    ledc::{
        LSGlobalClkSource, Ledc, LowSpeed,
        channel::{self, ChannelIFace},
        timer::{self, TimerIFace},
    },
    spi::master::Spi,
    time::{Instant, Rate},
};
use esp_println::println;
use esp32_s3_touch_lcd_1_28::{
    board,
    test_card::{draw_rotation_marker, draw_test_card},
};
use gc9a01::{
    Brightness, Gc9a01, SPIDisplayInterface, display::DisplayResolution240x240,
    rotation::DisplayRotation,
};

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

const MODE: &str = "blocking";

/// Terminal failure: print the FAIL marker once and idle forever, so the HIL
/// harness cannot mistake a later marker for success.
fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("GC9A01-REF mode={MODE} step={step} FAIL {err:?}");
    let mut delay = Delay::new();
    loop {
        delay.delay_ms(1000);
    }
}

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    println!("GC9A01-REF mode={MODE} step=boot gc9a01=0.5.0");

    let mut delay = Delay::new();

    // esp-hal's `Spi<'_, Blocking>` implements `SpiBus` but not
    // `SpiDevice`, so chip-select is a plain output pin and
    // `ExclusiveDevice` provides the `SpiDevice` the display interface
    // requires.
    let spi_bus = Spi::new(peripherals.SPI2, board::lcd_spi_config())
        .unwrap_or_else(|e| fail("spi", &e))
        .with_sck(peripherals.GPIO10)
        .with_mosi(peripherals.GPIO11);
    let cs = Output::new(peripherals.GPIO9, Level::High, OutputConfig::default());
    let spi = embedded_hal_bus::spi::ExclusiveDevice::new(spi_bus, cs, Delay::new())
        .unwrap_or_else(|e| fail("spi", &e));

    let dc = Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default());
    let mut rst = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
    // Backlight: LEDC PWM on GPIO2, starting at 0% (off until the panel is
    // initialized, as the old firmware did). PWM because the GC9A01's own
    // brightness register only drives its LEDPWM pin, which is not wired to
    // this board's backlight circuit — phase (d) sweeps both, but only the
    // GPIO2 PWM is visible here.
    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);
    let mut bl_timer = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    bl_timer
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty8Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_khz(5),
        })
        .unwrap_or_else(|e| fail("basic", &e));
    let mut bl = ledc.channel(channel::Number::Channel0, peripherals.GPIO2);
    bl.configure(channel::config::Config {
        timer: &bl_timer,
        duty_pct: 0,
        drive_mode: DriveMode::PushPull,
    })
    .unwrap_or_else(|e| fail("basic", &e));

    let iface = SPIDisplayInterface::new(spi, dc);
    let mut display = Gc9a01::new(iface, DisplayResolution240x240, DisplayRotation::Rotate180);
    display
        .reset(&mut rst, &mut delay)
        .unwrap_or_else(|e| fail("basic", &e));
    display
        .init(&mut delay)
        .unwrap_or_else(|e| fail("basic", &e));
    bl.set_duty(100).unwrap_or_else(|e| fail("basic", &e));

    // BasicMode phase: raw checkerboard, 16px cells, written straight to
    // the panel (no framebuffer involved yet).
    display.clear().unwrap_or_else(|e| fail("basic", &e));
    let mut checkerboard = (0..240u32 * 240).map(|i| {
        let (x, y) = (i % 240, i / 240);
        if (x / 16 + y / 16) % 2 == 0 {
            0xF800u16
        } else {
            0x0000
        }
    });
    display
        .set_pixels((0, 0), (239, 239), &mut checkerboard)
        .unwrap_or_else(|e| fail("basic", &e));
    println!("GC9A01-REF mode={MODE} step=basic-ok");
    delay.delay_ms(800);

    // Buffered phase, looping forever so a late monitor attach still sees a
    // full pass.
    let mut display = display.into_buffered_graphics_with_buffer(board::framebuffer());
    let mut cycle: u32 = 0;
    loop {
        // (a) Full-screen fills via the fast path, with flush timings.
        for (name, raw) in [
            ("RED", 0xF800u16),
            ("GREEN", 0x07E0),
            ("BLUE", 0x001F),
            ("WHITE", 0xFFFF),
            ("BLACK", 0x0000),
        ] {
            display.fill(raw);
            let t0 = Instant::now();
            display.flush().unwrap_or_else(|e| fail("fill", &e));
            println!(
                "GC9A01-REF mode={MODE} step=fill color={name} flush_ms={}",
                t0.elapsed().as_millis()
            );
            delay.delay_ms(300);
        }

        // (b) The reference test card (identical pixels in both bins).
        draw_test_card(&mut display, "BLOCKING").unwrap_or_else(|e| fail("card", &e));
        let t0 = Instant::now();
        display.flush().unwrap_or_else(|e| fail("card", &e));
        println!(
            "GC9A01-REF mode={MODE} step=card-ok flush_ms={}",
            t0.elapsed().as_millis()
        );
        delay.delay_ms(1500);

        // (c) All four rotations, with an up-arrow marker each.
        for (rotation, degrees, label) in [
            (DisplayRotation::Rotate0, 0u16, "ROT0"),
            (DisplayRotation::Rotate90, 90, "ROT90"),
            (DisplayRotation::Rotate180, 180, "ROT180"),
            (DisplayRotation::Rotate270, 270, "ROT270"),
        ] {
            display
                .set_display_rotation(rotation)
                .unwrap_or_else(|e| fail("rotation", &e));
            draw_rotation_marker(&mut display, label).unwrap_or_else(|e| fail("rotation", &e));
            display.flush().unwrap_or_else(|e| fail("rotation", &e));
            println!("GC9A01-REF mode={MODE} step=rotation value={degrees}-ok");
            delay.delay_ms(700);
        }
        display
            .set_display_rotation(DisplayRotation::Rotate180)
            .unwrap_or_else(|e| fail("rotation", &e));

        // (d) Brightness sweep with the card on screen. The driver's
        // `set_brightness` is exercised (its LEDPWM output is unwired on
        // this board, see backlight setup), and the same level is mirrored
        // to the GPIO2 PWM duty (DBV/255) so the sweep is actually visible.
        draw_test_card(&mut display, "BLOCKING").unwrap_or_else(|e| fail("brightness", &e));
        display.flush().unwrap_or_else(|e| fail("brightness", &e));
        for brightness in [
            Brightness::DIMMEST,
            Brightness::DIM,
            Brightness::NORMAL,
            Brightness::BRIGHTEST,
        ] {
            display
                .set_brightness(brightness)
                .unwrap_or_else(|e| fail("brightness", &e));
            let duty_pct = (brightness.brightness() as u32 * 100 / 255) as u8;
            bl.set_duty(duty_pct)
                .unwrap_or_else(|e| fail("brightness", &e));
            delay.delay_ms(600);
        }
        display
            .set_brightness(Brightness::NORMAL)
            .unwrap_or_else(|e| fail("brightness", &e));
        bl.set_duty(100).unwrap_or_else(|e| fail("brightness", &e));
        println!("GC9A01-REF mode={MODE} step=brightness-ok");

        // (e) First-cycle ALL-PASS is the HIL success token.
        println!("GC9A01-REF mode={MODE} cycle={cycle} result=ALL-PASS");
        cycle += 1;
        delay.delay_ms(1000);
    }
}
