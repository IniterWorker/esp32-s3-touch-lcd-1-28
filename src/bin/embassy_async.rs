//! Embassy async reference test for gc9a01-rs 0.5 on the
//! ESP32-S3-Touch-LCD-1.28.
//!
//! Runs the reference sequence against the async `Gc9a01Async` driver and
//! prints `GC9A01-REF mode=async ...` markers on UART; `scripts/hil.sh`
//! greps them. The sequence, markers, delays and flush timings are
//! identical to `blocking.rs` (only the mode string, `.await`s, delay type
//! and the heartbeat task differ) — keep them that way.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Instant, Timer};
use esp_hal::{
    clock::CpuClock,
    dma_rx_buffer, dma_tx_buffer,
    gpio::{Level, Output, OutputConfig},
    peripherals::{DMA_CH0, GPIO2, GPIO8, GPIO9, GPIO10, GPIO11, GPIO14, SPI2},
    spi::master::Spi,
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp32_s3_touch_lcd_1_28::{
    board,
    test_card::{draw_rotation_marker, draw_test_card},
};
use gc9a01::{
    AsyncSpiInterface, Brightness, Gc9a01Async, display::DisplayResolution240x240,
    rotation::DisplayRotation,
};

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

const MODE: &str = "async";

/// Terminal failure: print the FAIL marker once and idle forever, so the HIL
/// harness cannot mistake a later marker for success.
async fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("GC9A01-REF mode={MODE} step={step} FAIL {err:?}");
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Executor-liveness proof: heartbeats must keep interleaving with the
/// display task's `flush_ms` lines while DMA flushes run. A
/// blocking-in-async regression shows up as heartbeat gaps > 2 s during
/// fills; `scripts/hil.sh` checks for a heartbeat between the first fill
/// and `ALL-PASS`.
#[embassy_executor::task]
async fn heartbeat_task() {
    let start = Instant::now();
    loop {
        Timer::after(Duration::from_secs(1)).await;
        println!(
            "GC9A01-REF mode={MODE} step=heartbeat uptime_s={}",
            start.elapsed().as_secs()
        );
    }
}

/// The peripherals the LCD owns, bundled so the task takes one argument:
/// SPI2, DMA channel, then SCK/MOSI/CS/DC/RST/BL pins.
type LcdPeripherals = (
    SPI2<'static>,
    DMA_CH0<'static>,
    GPIO10<'static>,
    GPIO11<'static>,
    GPIO9<'static>,
    GPIO8<'static>,
    GPIO14<'static>,
    GPIO2<'static>,
);

/// Owns the SPI bus and display; runs the reference sequence (step-for-step
/// identical to the blocking bin) forever.
#[embassy_executor::task]
async fn display_task(p: LcdPeripherals) {
    let (spi2, dma, sck, mosi, cs, dc_pin, rst_pin, bl_pin) = p;
    let dma_rx_buf = match dma_rx_buffer!(8192) {
        Ok(buf) => buf,
        Err(e) => fail("spi", &e).await,
    };
    let dma_tx_buf = match dma_tx_buffer!(8192) {
        Ok(buf) => buf,
        Err(e) => fail("spi", &e).await,
    };

    // esp-hal's async DMA `SpiDma<'_, Async>` implements `SpiBus` but not
    // `SpiDevice`, so chip-select is a plain output pin and the async
    // `ExclusiveDevice` provides the `SpiDevice` the display interface
    // requires (same for the blocking bin).
    let spi_bus = match Spi::new(spi2, board::lcd_spi_config()) {
        Ok(spi) => spi,
        Err(e) => fail("spi", &e).await,
    }
    .with_sck(sck)
    .with_mosi(mosi)
    .with_dma(dma)
    .with_buffers(dma_rx_buf, dma_tx_buf)
    .into_async();
    let cs = Output::new(cs, Level::High, OutputConfig::default());
    let spi = match embedded_hal_bus::spi::ExclusiveDevice::new(spi_bus, cs, Delay) {
        Ok(spi) => spi,
        Err(e) => fail("spi", &e).await,
    };

    let dc = Output::new(dc_pin, Level::Low, OutputConfig::default());
    let mut rst = Output::new(rst_pin, Level::High, OutputConfig::default());
    // Backlight stays off until the panel is initialized (old firmware order).
    let mut bl = Output::new(bl_pin, Level::Low, OutputConfig::default());

    let iface = AsyncSpiInterface::new(spi, dc);
    let mut display = Gc9a01Async::new(iface, DisplayResolution240x240, DisplayRotation::Rotate180);
    if let Err(e) = display.reset(&mut rst, &mut Delay).await {
        fail("basic", &e).await;
    }
    if let Err(e) = display.init(&mut Delay).await {
        fail("basic", &e).await;
    }
    bl.set_high();

    // BasicMode phase: raw checkerboard, 16px cells, written straight to
    // the panel (no framebuffer involved yet).
    if let Err(e) = display.clear().await {
        fail("basic", &e).await;
    }
    let mut checkerboard = (0..240u32 * 240).map(|i| {
        let (x, y) = (i % 240, i / 240);
        if (x / 16 + y / 16) % 2 == 0 {
            0xF800u16
        } else {
            0x0000
        }
    });
    if let Err(e) = display
        .set_pixels((0, 0), (239, 239), &mut checkerboard)
        .await
    {
        fail("basic", &e).await;
    }
    println!("GC9A01-REF mode={MODE} step=basic-ok");
    Timer::after(Duration::from_millis(800)).await;

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
            if let Err(e) = display.flush().await {
                fail("fill", &e).await;
            }
            println!(
                "GC9A01-REF mode={MODE} step=fill color={name} flush_ms={}",
                t0.elapsed().as_millis()
            );
            Timer::after(Duration::from_millis(300)).await;
        }

        // (b) The reference test card (identical pixels in both bins).
        if let Err(e) = draw_test_card(&mut display, "ASYNC") {
            fail("card", &e).await;
        }
        let t0 = Instant::now();
        if let Err(e) = display.flush().await {
            fail("card", &e).await;
        }
        println!(
            "GC9A01-REF mode={MODE} step=card-ok flush_ms={}",
            t0.elapsed().as_millis()
        );
        Timer::after(Duration::from_millis(1500)).await;

        // (c) All four rotations, with an up-arrow marker each.
        for (rotation, degrees, label) in [
            (DisplayRotation::Rotate0, 0u16, "ROT0"),
            (DisplayRotation::Rotate90, 90, "ROT90"),
            (DisplayRotation::Rotate180, 180, "ROT180"),
            (DisplayRotation::Rotate270, 270, "ROT270"),
        ] {
            if let Err(e) = display.set_display_rotation(rotation).await {
                fail("rotation", &e).await;
            }
            if let Err(e) = draw_rotation_marker(&mut display, label) {
                fail("rotation", &e).await;
            }
            if let Err(e) = display.flush().await {
                fail("rotation", &e).await;
            }
            println!("GC9A01-REF mode={MODE} step=rotation value={degrees}-ok");
            Timer::after(Duration::from_millis(700)).await;
        }
        if let Err(e) = display
            .set_display_rotation(DisplayRotation::Rotate180)
            .await
        {
            fail("rotation", &e).await;
        }

        // (d) Controller brightness sweep with the card on screen.
        if let Err(e) = draw_test_card(&mut display, "ASYNC") {
            fail("brightness", &e).await;
        }
        if let Err(e) = display.flush().await {
            fail("brightness", &e).await;
        }
        for brightness in [
            Brightness::DIMMEST,
            Brightness::DIM,
            Brightness::NORMAL,
            Brightness::BRIGHTEST,
        ] {
            if let Err(e) = display.set_brightness(brightness).await {
                fail("brightness", &e).await;
            }
            Timer::after(Duration::from_millis(300)).await;
        }
        if let Err(e) = display.set_brightness(Brightness::NORMAL).await {
            fail("brightness", &e).await;
        }
        println!("GC9A01-REF mode={MODE} step=brightness-ok");

        // (e) First-cycle ALL-PASS is the HIL success token.
        println!("GC9A01-REF mode={MODE} cycle={cycle} result=ALL-PASS");
        cycle += 1;
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    println!("GC9A01-REF mode={MODE} step=boot gc9a01=0.5.0");

    spawner.spawn(heartbeat_task().unwrap());
    spawner.spawn(
        display_task((
            peripherals.SPI2,
            peripherals.DMA_CH0,
            peripherals.GPIO10,
            peripherals.GPIO11,
            peripherals.GPIO9,
            peripherals.GPIO8,
            peripherals.GPIO14,
            peripherals.GPIO2,
        ))
        .unwrap(),
    );

    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}
