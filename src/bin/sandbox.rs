//! SBX sandbox: full-board Embassy async application on rlvgl.
//!
//! Touch UI (6 pages) + UART line protocol (`SBX GET/SET/STREAM`) +
//! settings persisted in the `settings` flash partition. Markers on
//! UART: `SBX step=boot`, `SBX psram_bytes=<n>`,
//! `SBX step=settings-loaded seq=<n>`, `SBX step=touch-ok`,
//! `SBX step=imu-ok`, `SBX step=ui-ok` (first frame flushed),
//! `SBX step=settings-saved seq=<n>`, `SBX step=sleep/wake`.
//!
//! Task map: `ui_task` owns display + widget tree + AppState (the
//! only mutator, via `AppState::apply`); `sensor_task` owns the
//! shared I2C bus (touch + IMU); `uart_task` parses the line protocol
//! into `Cmd`s; `settings_task` owns flash writes (debounced);
//! `battery_task` samples the ADC; `button_task` watches BOOT (GPIO0).

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::rc::Rc;
use alloc::string::String;
use core::cell::RefCell;

use cst816s::{Cst816sAsync, config::ReportMode};
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex as Cs, channel::Channel, signal::Signal,
    watch::Watch,
};
use embassy_time::{Delay, Duration, Instant, Ticker, Timer, with_timeout};
use esp_hal::{
    analog::adc::{Adc, AdcConfig, Attenuation},
    clock::CpuClock,
    dma_rx_buffer, dma_tx_buffer,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::I2c,
    ledc::channel::ChannelIFace,
    peripherals::{
        ADC1, DMA_CH0, FLASH, GPIO0, GPIO1, GPIO2, GPIO5, GPIO6, GPIO7, GPIO8, GPIO9, GPIO10,
        GPIO11, GPIO13, GPIO14, GPIO43, GPIO44, I2C0, LEDC, SPI2, UART0,
    },
    spi::master::Spi,
    timer::timg::TimerGroup,
    uart::{Config as UartConfig, Uart},
};
use esp_println::println;
use esp_storage::FlashStorage;
use esp32_s3_touch_lcd_1_28::{
    board::{
        self,
        shared_i2c::{SharedI2c, SharedI2cBus},
    },
    settings::{Settings, SettingsStore},
    ui::{
        app::SandboxApp,
        display_backend::framebuffer_surface,
        input::{Gesture, GestureTracker, TouchReport as UiTouch, rotate_touch},
        state::{AppState, ImuSample},
    },
};
use gc9a01::{
    AsyncSpiInterface, Gc9a01Async, display::DisplayResolution240x240, rotation::DisplayRotation,
};
use qmi8658::{
    DeviceAddress, Qmi8658Async,
    config::{
        AccelConfig, Config as ImuConfig, GyroConfig, MotionConfig, MotionEngines, PedometerConfig,
        TapConfig,
    },
    registers::{AccelOdr, Odr},
};
use rlvgl_core::application::Application;
use rlvgl_platform::{CpuBlitter, blit::BlitterRenderer};

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

// ---------------------------------------------------------------------
// Inter-task plumbing (statics; one sandbox per boot).
// ---------------------------------------------------------------------

/// UART-originated requests, executed by ui_task (sole state owner).
enum Cmd {
    Set(String, String),
    Get,
}

/// Sensor enable/disable + ODR control, ui_task → sensor_task.
#[derive(Clone, Copy)]
enum SensorCtrl {
    Imu(bool),
    Pedometer(bool),
    Odr(u8),
}

/// BOOT-button navigation, button_task → ui_task.
#[derive(Clone, Copy)]
enum Nav {
    NextPage,
    FactoryReset,
}

/// Snapshot of everything the sensor hub publishes.
#[derive(Clone, Copy, Default)]
struct SensorState {
    imu: Option<ImuSample>,
    steps: u32,
    touch_ok: bool,
    imu_ok: bool,
    pedometer_supported: bool,
    touch_proj: u8,
    imu_rev: u8,
    last_event: &'static str,
}

static CMD_CH: Channel<Cs, Cmd, 4> = Channel::new();
static NAV_CH: Channel<Cs, Nav, 2> = Channel::new();
static CTRL_CH: Channel<Cs, SensorCtrl, 4> = Channel::new();
static SAVE_CH: Channel<Cs, Settings, 1> = Channel::new();
static TOUCH_CH: Channel<Cs, UiTouch, 8> = Channel::new();
static LOADED: Signal<Cs, (u32, Settings)> = Signal::new();
static BATT_WATCH: Watch<Cs, u32, 1> = Watch::new();
/// Last saved settings seq, settings_task → ui_task (system page).
static SEQ_WATCH: Watch<Cs, u32, 1> = Watch::new();
static SENSOR_WATCH: Watch<Cs, SensorState, 1> = Watch::new();
static I2C_BUS: static_cell::StaticCell<SharedI2cBus<I2c<'static, esp_hal::Async>>> =
    static_cell::StaticCell::new();

async fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("SBX step={step} result=FAIL err={err:?}");
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

fn rotation_of(idx: u8) -> DisplayRotation {
    match idx % 4 {
        0 => DisplayRotation::Rotate0,
        1 => DisplayRotation::Rotate90,
        2 => DisplayRotation::Rotate180,
        _ => DisplayRotation::Rotate270,
    }
}

/// IMU config for a settings ODR index (accel ±4 g, gyro ±512 dps
/// from the driver defaults; ODR 112/224/448/897 Hz).
fn imu_config(odr_idx: u8) -> ImuConfig {
    let odr = match odr_idx % 4 {
        0 => Odr::Hz112,
        1 => Odr::Hz224,
        2 => Odr::Hz448,
        _ => Odr::Hz897,
    };
    ImuConfig {
        accel: Some(AccelConfig {
            odr: AccelOdr::SixDof(odr),
            ..AccelConfig::default()
        }),
        gyro: Some(GyroConfig {
            odr,
            ..GyroConfig::default()
        }),
        sync_sample: false,
    }
}

fn to_sample(s: &qmi8658::data::Sample) -> ImuSample {
    let mg = |g: f32| (g * 1000.0) as i32;
    let mdps = |d: f32| (d * 1000.0) as i32;
    ImuSample {
        ax_mg: mg(s.accel_g[0]),
        ay_mg: mg(s.accel_g[1]),
        az_mg: mg(s.accel_g[2]),
        gx_mdps: mdps(s.gyro_dps[0]),
        gy_mdps: mdps(s.gyro_dps[1]),
        gz_mdps: mdps(s.gyro_dps[2]),
        temp_dc: (s.temperature_c * 10.0) as i32,
    }
}

// ---------------------------------------------------------------------
// settings_task: owns the flash store.
// ---------------------------------------------------------------------

#[embassy_executor::task]
async fn settings_task(flash_periph: FLASH<'static>) {
    let mut store = match SettingsStore::open(FlashStorage::new(flash_periph)) {
        Some(s) => s,
        None => {
            println!("SBX step=settings-unavailable");
            LOADED.signal((0, Settings::default()));
            loop {
                Timer::after(Duration::from_secs(3600)).await;
            }
        }
    };
    let (seq, settings) = store.load();
    if store.has_stored() {
        println!("SBX step=settings-loaded seq={seq}");
    } else {
        println!("SBX step=settings-loaded seq=defaults");
    }
    SEQ_WATCH.sender().send(seq);
    LOADED.signal((seq, settings));

    loop {
        let mut pending = SAVE_CH.receive().await;
        while let Ok(next) = with_timeout(Duration::from_millis(500), SAVE_CH.receive()).await {
            pending = next;
        }
        match store.save(&pending) {
            Ok(seq) => {
                SEQ_WATCH.sender().send(seq);
                println!("SBX step=settings-saved seq={seq}");
            }
            Err(e) => println!("SBX step=settings-save-failed err={e:?}"),
        }
    }
}

// ---------------------------------------------------------------------
// uart_task: SBX line protocol on UART0 (CH343 bridge, GPIO43/44).
// ---------------------------------------------------------------------

#[embassy_executor::task]
async fn uart_task(uart0: UART0<'static>, tx: GPIO43<'static>, rx: GPIO44<'static>) {
    let uart = match Uart::new(uart0, UartConfig::default()) {
        Ok(u) => u.with_tx(tx).with_rx(rx).into_async(),
        Err(e) => fail("uart", &e).await,
    };
    let (mut uart_rx, _uart_tx) = uart.split();
    let mut line: String = String::with_capacity(128);
    let mut buf = [0u8; 32];
    loop {
        match uart_rx.read_async(&mut buf).await {
            Ok(n) => {
                for &b in &buf[..n] {
                    match b {
                        b'\n' => {
                            handle_line(line.trim());
                            line.clear();
                        }
                        b'\r' => {}
                        _ => {
                            if line.len() < 127 {
                                line.push(b as char);
                            }
                        }
                    }
                }
            }
            Err(_) => Timer::after(Duration::from_millis(10)).await,
        }
    }
}

fn handle_line(line: &str) {
    let Some(rest) = line.strip_prefix("SBX ") else {
        return; // monitor noise / echoes: stay silent
    };
    match rest {
        "GET" => {
            let _ = CMD_CH.try_send(Cmd::Get);
        }
        "STREAM on" => {
            let _ = CMD_CH.try_send(Cmd::Set(String::from("stream"), String::from("on")));
        }
        "STREAM off" => {
            let _ = CMD_CH.try_send(Cmd::Set(String::from("stream"), String::from("off")));
        }
        _ => {
            if let Some(kv) = rest.strip_prefix("SET ")
                && let Some((k, v)) = kv.split_once('=')
            {
                let _ = CMD_CH.try_send(Cmd::Set(String::from(k.trim()), String::from(v.trim())));
            }
        }
    }
}

// ---------------------------------------------------------------------
// sensor_task: shared I2C — CST816S touch + QMI8658 IMU.
// ---------------------------------------------------------------------

type SensorPeripherals = (
    I2C0<'static>,
    GPIO6<'static>,
    GPIO7<'static>,
    GPIO13<'static>,
    GPIO5<'static>,
);

#[embassy_executor::task]
async fn sensor_task(p: SensorPeripherals) {
    let (i2c0, sda, scl, tp_rst, tp_int) = p;
    let i2c = match I2c::new(i2c0, board::i2c_config()) {
        Ok(i) => i.with_sda(sda).with_scl(scl).into_async(),
        Err(e) => fail("i2c", &e).await,
    };
    let bus: &'static SharedI2cBus<I2c<'static, esp_hal::Async>> =
        I2C_BUS.init(embassy_sync::mutex::Mutex::new(i2c));

    let rst = Output::new(tp_rst, Level::High, OutputConfig::default());
    let int = Input::new(tp_int, InputConfig::default().with_pull(Pull::Up));
    let mut touch = Some(
        Cst816sAsync::new(SharedI2c(bus), Delay)
            .with_reset_pin(rst)
            .with_interrupt_pin(int),
    );
    let mut imu = Some(Qmi8658Async::new_i2c(
        SharedI2c(bus),
        DeviceAddress::Secondary,
        Delay,
    ));

    let mut ss = SensorState {
        last_event: "-",
        ..SensorState::default()
    };
    let sender = SENSOR_WATCH.sender();

    // Touch probe + the proven proj105 config (see touch_async bin).
    if let Some(t) = touch.as_mut() {
        match t.init().await {
            Ok(info) => {
                println!(
                    "SBX step=touch-ok chip={:?} proj={} fw={}",
                    info.chip, info.project_id, info.firmware_version
                );
                ss.touch_ok = true;
                ss.touch_proj = info.project_id;
                let _ = t.set_irq_pulse_width(20).await;
                let _ = t.set_auto_reset(0).await;
                let _ = t.set_report_mode(ReportMode::PointAndGesture).await;
                let _ = t.set_disable_auto_sleep(true).await;
                let _ = t
                    .set_motion_mask(cst816s::config::MotionMask {
                        continuous_left_right: true,
                        continuous_up_down: true,
                        double_click: true,
                    })
                    .await;
            }
            Err(e) => {
                println!("SBX step=touch-fail err={e:?}");
                touch = None;
            }
        }
    }

    // IMU probe + engines. ≥15 ms after power-up before init.
    Timer::after(Duration::from_millis(20)).await;
    let mut pedometer_want = true;
    if let Some(m) = imu.as_mut() {
        match m.init(imu_config(0)).await {
            Ok(()) => {
                if let Ok(id) = m.device_id().await {
                    ss.imu_rev = id.revision;
                }
                println!("SBX step=imu-ok rev=0x{:02x}", ss.imu_rev);
                ss.imu_ok = true;
                let _ = m.configure_tap(&TapConfig::default(), true).await;
                let _ = m
                    .configure_motion(
                        &MotionConfig::default(),
                        MotionEngines {
                            any_motion: true,
                            no_motion: false,
                            significant_motion: false,
                        },
                    )
                    .await;
                match m
                    .configure_pedometer(&PedometerConfig::default(), true)
                    .await
                {
                    Ok(()) => ss.pedometer_supported = true,
                    Err(e) => println!("SBX step=pedometer-unsupported err={e:?}"),
                }
                Timer::after(Duration::from_millis(250)).await;
            }
            Err(e) => {
                println!("SBX step=imu-fail err={e:?}");
                imu = None;
            }
        }
    }
    sender.send(ss);

    let mut imu_enabled = true;
    let mut was_pressed = false;
    let mut step_ticks: u32 = 0;
    loop {
        // Wait for a touch interrupt, a control message, or the
        // 50 ms IMU cadence — whichever comes first.
        enum Ev {
            Touch,
            Ctrl(SensorCtrl),
            Tick,
        }
        let ev = {
            let touch_fut = async {
                match touch.as_mut() {
                    Some(t) => {
                        let _ =
                            with_timeout(Duration::from_secs(3600), t.wait_for_interrupt()).await;
                    }
                    _ => Timer::after(Duration::from_secs(3600)).await,
                }
                Ev::Touch
            };
            let ctrl_fut = async { Ev::Ctrl(CTRL_CH.receive().await) };
            let tick_fut = async {
                Timer::after(Duration::from_millis(50)).await;
                Ev::Tick
            };
            match select3(touch_fut, ctrl_fut, tick_fut).await {
                Either3::First(e) | Either3::Second(e) | Either3::Third(e) => e,
            }
        };

        match ev {
            Ev::Ctrl(c) => {
                match c {
                    SensorCtrl::Imu(on) => {
                        imu_enabled = on;
                        if let Some(m) = imu.as_mut() {
                            let _ = m.set_sensors_enabled(on, on).await;
                        }
                        if !on {
                            ss.imu = None;
                            sender.send(ss);
                        }
                    }
                    SensorCtrl::Pedometer(on) => {
                        pedometer_want = on;
                        if let Some(m) = imu.as_mut() {
                            let _ = m.configure_pedometer(&PedometerConfig::default(), on).await;
                        }
                    }
                    SensorCtrl::Odr(idx) => {
                        if let Some(m) = imu.as_mut() {
                            let _ = m.apply_config(imu_config(idx)).await;
                        }
                    }
                }
                continue;
            }
            Ev::Touch | Ev::Tick => {}
        }

        // Touch: on INT (or, while pressed, on ticks so releases are
        // seen even if the module stays quiet) read one report.
        if let Some(t) = touch.as_mut()
            && (matches!(ev, Ev::Touch) || was_pressed)
            && let Ok(r) = t.read_touch().await
        {
            was_pressed = r.pressed;
            let _ = TOUCH_CH.try_send(UiTouch {
                x: i32::from(r.x),
                y: i32::from(r.y),
                pressed: r.pressed,
            });
        }

        // IMU on the tick cadence only.
        if matches!(ev, Ev::Tick)
            && imu_enabled
            && let Some(m) = imu.as_mut()
        {
            let mut dirty = false;
            if let Ok(s) = m.read_sample().await {
                ss.imu = Some(to_sample(&s));
                dirty = true;
            }
            if let Ok(a) = m.activity_status().await
                && a.any()
            {
                ss.last_event = if a.tap { "TAP" } else { "MOTION" };
                dirty = true;
            }
            step_ticks = step_ticks.wrapping_add(1);
            if pedometer_want && ss.pedometer_supported && step_ticks.is_multiple_of(20) {
                if let Ok(n) = m.step_count().await {
                    ss.steps = n;
                }
                dirty = true;
            }
            if dirty {
                sender.send(ss);
            }
        }
    }
}

// ---------------------------------------------------------------------
// battery_task: ADC1/GPIO1, 8-read average every 2 s.
// ---------------------------------------------------------------------

#[embassy_executor::task]
async fn battery_task(adc1: ADC1<'static>, pin: GPIO1<'static>) {
    let mut adc_config = AdcConfig::new();
    let mut adc_pin = adc_config.enable_pin(pin, Attenuation::_11dB);
    let mut adc = Adc::new(adc1, adc_config).into_async();
    let sender = BATT_WATCH.sender();
    loop {
        let mut sum: u32 = 0;
        for _ in 0..8 {
            sum += u32::from(adc.read_oneshot(&mut adc_pin).await);
        }
        sender.send(esp32_s3_touch_lcd_1_28::battery::battery_mv(
            (sum / 8) as u16,
        ));
        Timer::after(Duration::from_secs(2)).await;
    }
}

// ---------------------------------------------------------------------
// button_task: BOOT (GPIO0) — short press next page, 2 s hold reset.
// ---------------------------------------------------------------------

#[embassy_executor::task]
async fn button_task(pin: GPIO0<'static>) {
    let mut btn = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
    loop {
        btn.wait_for_falling_edge().await;
        let t0 = Instant::now();
        // Long hold fires while still pressed.
        let released_early =
            with_timeout(Duration::from_millis(2000), btn.wait_for_rising_edge()).await;
        if released_early.is_err() {
            let _ = NAV_CH.try_send(Nav::FactoryReset);
            btn.wait_for_rising_edge().await;
        } else if t0.elapsed() < Duration::from_millis(800) {
            let _ = NAV_CH.try_send(Nav::NextPage);
        }
        Timer::after(Duration::from_millis(50)).await; // debounce
    }
}

// ---------------------------------------------------------------------
// ui_task: display + rlvgl + AppState (sole state mutator).
// ---------------------------------------------------------------------

type UiPeripherals = (
    SPI2<'static>,
    DMA_CH0<'static>,
    GPIO10<'static>,
    GPIO11<'static>,
    GPIO9<'static>,
    GPIO8<'static>,
    GPIO14<'static>,
    GPIO2<'static>,
    LEDC<'static>,
);

/// Heading integration sign. Gyro +Z vs panel-up yaw direction;
/// determined empirically — flip here if the needle runs backwards.
const YAW_SIGN: i32 = 1;

#[allow(clippy::too_many_lines)]
#[embassy_executor::task]
async fn ui_task(p: UiPeripherals, psram_bytes: usize) {
    let (spi2, dma, sck, mosi, cs_pin, dc_pin, rst_pin, bl_pin, ledc_periph) = p;
    let dma_rx_buf = match dma_rx_buffer!(8192) {
        Ok(b) => b,
        Err(e) => fail("spi", &e).await,
    };
    let dma_tx_buf = match dma_tx_buffer!(8192) {
        Ok(b) => b,
        Err(e) => fail("spi", &e).await,
    };
    let spi_bus = match Spi::new(spi2, board::lcd_spi_config()) {
        Ok(s) => s,
        Err(e) => fail("spi", &e).await,
    }
    .with_sck(sck)
    .with_mosi(mosi)
    .with_dma(dma)
    .with_buffers(dma_rx_buf, dma_tx_buf)
    .into_async();
    let cs = Output::new(cs_pin, Level::High, OutputConfig::default());
    let spi = match embedded_hal_bus::spi::ExclusiveDevice::new(spi_bus, cs, Delay) {
        Ok(s) => s,
        Err(e) => fail("spi", &e).await,
    };
    let dc = Output::new(dc_pin, Level::Low, OutputConfig::default());
    let mut rst = Output::new(rst_pin, Level::High, OutputConfig::default());
    let bl = match board::backlight_pwm(ledc_periph, bl_pin) {
        Ok(v) => v,
        Err(e) => fail("basic", &e).await,
    };

    let (seq, settings) = with_timeout(Duration::from_secs(2), LOADED.wait())
        .await
        .unwrap_or((0, Settings::default()));
    // Tell the sensor hub the loaded feature set.
    let _ = CTRL_CH.try_send(SensorCtrl::Imu(settings.imu_enabled));
    let _ = CTRL_CH.try_send(SensorCtrl::Pedometer(settings.pedometer));
    let _ = CTRL_CH.try_send(SensorCtrl::Odr(settings.imu_odr_idx));

    let iface = AsyncSpiInterface::new(spi, dc);
    let mut display = Gc9a01Async::new(
        iface,
        DisplayResolution240x240,
        rotation_of(settings.rotation),
    );
    if let Err(e) = display.reset(&mut rst, &mut Delay).await {
        fail("basic", &e).await;
    }
    if let Err(e) = display.init(&mut Delay).await {
        fail("basic", &e).await;
    }
    // Panel inversion polarity: gc9a01 init() leaves INVON set, and
    // this module needs INVON for correct color (INVOFF renders the
    // dark UI near-white). User-facing `invert` is therefore the
    // negation of the register bit: invert=false → INVON (normal).
    if let Err(e) = display.set_invert_pixels(!settings.invert).await {
        fail("basic", &e).await;
    }
    if let Err(e) = bl.set_duty(settings.backlight_pct) {
        fail("basic", &e).await;
    }
    let mut display = display.into_buffered_graphics_with_buffer(board::framebuffer());

    // rlvgl render target (`vec!` allocates in place; see step 6 note).
    let mut surface_fb = alloc::vec![0u16; 240 * 240];
    let mut blitter = CpuBlitter;
    let mut app_state = AppState::new(settings, seq);
    app_state.psram_bytes = psram_bytes;
    let mut app = SandboxApp::new(app_state);
    let root = Rc::new(RefCell::new(app.build(240, 240)));

    let mut batt_rx = BATT_WATCH.receiver();
    let mut seq_rx = SEQ_WATCH.receiver();
    let mut sensor_rx = SENSOR_WATCH.receiver();
    let mut tracker = GestureTracker::default();
    let boot = Instant::now();
    let mut ticker = Ticker::every(Duration::from_millis(16));
    let mut last_flush = Instant::now();
    let mut flushed_once = false;
    let mut last_rev = u32::MAX;
    let mut last_touch_at = Instant::now();
    let mut sleeping = false;
    let mut last_stream = Instant::now();
    let mut last_imu_at = Instant::now();
    let mut applied = settings;
    loop {
        ticker.next().await;
        app.state.uptime_s = boot.elapsed().as_secs() as u32;

        // --- telemetry ingest ---
        if let Some(rx) = batt_rx.as_mut()
            && let Some(mv) = rx.try_changed()
        {
            app.state.battery_mv = mv;
        }
        if let Some(rx) = seq_rx.as_mut()
            && let Some(seq) = rx.try_changed()
        {
            app.state.settings_seq = seq;
        }
        if let Some(rx) = sensor_rx.as_mut()
            && let Some(s) = rx.try_changed()
        {
            // Heading: integrate gyro Z between fresh samples.
            if let Some(sample) = s.imu {
                let dt_ms = last_imu_at.elapsed().as_millis() as i64;
                if dt_ms > 0 && dt_ms < 1000 {
                    let delta = (i64::from(sample.gz_mdps) * dt_ms / 1_000_000) as i32 * YAW_SIGN;
                    app.state.heading_deg = (app.state.heading_deg + delta).rem_euclid(360);
                }
                last_imu_at = Instant::now();
            }
            app.state.imu = s.imu;
            app.state.steps = s.steps;
            app.state.touch_ok = s.touch_ok;
            app.state.imu_ok = s.imu_ok;
            app.state.pedometer_supported = s.pedometer_supported;
            app.state.touch_id = s.touch_proj;
            app.state.imu_rev = s.imu_rev;
            if s.last_event != "-" {
                app.state.last_event = s.last_event;
            }
        }

        // --- touch → widget events + gestures ---
        while let Ok(raw) = TOUCH_CH.try_receive() {
            // Touch follows display rotation: raw CST816 coords are
            // in the Rotate180 frame; map into current logical frame.
            let (x, y) = rotate_touch(raw.x, raw.y, app.state.settings.rotation);
            let r = UiTouch {
                x,
                y,
                pressed: raw.pressed,
            };
            if r.pressed {
                last_touch_at = Instant::now();
                app.state.last_touch = Some((r.x, r.y));
            }
            if sleeping {
                sleeping = false;
                let _ = bl.set_duty(app.state.settings.backlight_pct);
                println!("SBX step=wake");
            }
            let tick = boot.elapsed().as_millis() as u32 / 16;
            let (events, gesture) = tracker.feed(r, tick);
            for ev in &events {
                root.borrow_mut().dispatch_event(ev);
                app.after_event(&root, ev);
            }
            if let Some(g) = gesture {
                app.state.last_gesture = g.name();
                let h = app.state.gesture_history;
                app.state.gesture_history = [g.name(), h[0], h[1], h[2], h[3]];
                match g {
                    Gesture::SwipeLeft => {
                        let p =
                            (app.state.page + 1) % esp32_s3_touch_lcd_1_28::ui::state::PAGE_COUNT;
                        let _ = app.state.apply("page", &alloc::format!("{p}"));
                    }
                    Gesture::SwipeRight => {
                        let pc = esp32_s3_touch_lcd_1_28::ui::state::PAGE_COUNT;
                        let p = (app.state.page + pc - 1) % pc;
                        let _ = app.state.apply("page", &alloc::format!("{p}"));
                    }
                    Gesture::Tap { .. } if app.state.page == 2 => {
                        app.state.heading_deg = 0;
                    }
                    _ => {}
                }
            }
        }

        // --- BOOT button navigation ---
        while let Ok(nav) = NAV_CH.try_receive() {
            last_touch_at = Instant::now();
            if sleeping {
                sleeping = false;
                let _ = bl.set_duty(app.state.settings.backlight_pct);
                println!("SBX step=wake");
            }
            match nav {
                Nav::NextPage => {
                    let p = (app.state.page + 1) % esp32_s3_touch_lcd_1_28::ui::state::PAGE_COUNT;
                    let _ = app.state.apply("page", &alloc::format!("{p}"));
                }
                Nav::FactoryReset => {
                    app.state.settings = Settings::default();
                    app.state.rev = app.state.rev.wrapping_add(1);
                    println!("SBX step=settings-reset");
                }
            }
        }

        // --- UART commands ---
        while let Ok(cmd) = CMD_CH.try_receive() {
            match cmd {
                Cmd::Get => {
                    let mut buf = [0u8; 16];
                    for key in AppState::GET_KEYS {
                        if let Some(v) = app.state.get_str(key, &mut buf) {
                            println!("SBX-STATE {key}={v}");
                        }
                    }
                }
                Cmd::Set(k, v) => match app.state.apply(&k, &v) {
                    Ok(()) => println!("SBX OK {k}={v}"),
                    Err(_) => println!("SBX ERR {k}"),
                },
            }
        }

        app.tick(&root);

        // --- IMU stream (5 Hz) ---
        if app.state.settings.stream_imu && last_stream.elapsed() >= Duration::from_millis(200) {
            last_stream = Instant::now();
            if let Some(s) = app.state.imu {
                println!(
                    "SBX-DATA ax_mg={} ay_mg={} az_mg={} gx_mdps={} batt_mv={}",
                    s.ax_mg, s.ay_mg, s.az_mg, s.gx_mdps, app.state.battery_mv
                );
            }
        }

        // --- sleep timeout ---
        let timeout_s = u64::from(app.state.settings.sleep_timeout_s);
        if !sleeping && timeout_s > 0 && last_touch_at.elapsed().as_secs() >= timeout_s {
            sleeping = true;
            let _ = bl.set_duty(0);
            println!("SBX step=sleep");
        }

        // --- persistence hand-off + sensor ctrl + actuators ---
        let want = app.state.settings;
        if want != applied {
            if SAVE_CH.try_send(want).is_err() {
                SAVE_CH.clear();
                let _ = SAVE_CH.try_send(want);
            }
            if want.imu_enabled != applied.imu_enabled {
                let _ = CTRL_CH.try_send(SensorCtrl::Imu(want.imu_enabled));
            }
            if want.pedometer != applied.pedometer {
                let _ = CTRL_CH.try_send(SensorCtrl::Pedometer(want.pedometer));
            }
            if want.imu_odr_idx != applied.imu_odr_idx {
                let _ = CTRL_CH.try_send(SensorCtrl::Odr(want.imu_odr_idx));
            }
            if want.backlight_pct != applied.backlight_pct && !sleeping {
                let _ = bl.set_duty(want.backlight_pct);
            }
            if want.rotation != applied.rotation {
                let _ = display
                    .set_display_rotation(rotation_of(want.rotation))
                    .await;
                tracker.reset();
            }
            if want.invert != applied.invert {
                let _ = display.set_invert_pixels(!want.invert).await;
            }
            applied = want;
        }

        // --- render on rev change or 250 ms heartbeat ---
        let rev_changed = app.state.rev != last_rev;
        let heartbeat = last_flush.elapsed() >= Duration::from_millis(250);
        if !sleeping && (rev_changed || heartbeat) {
            last_rev = app.state.rev;
            last_flush = Instant::now();
            {
                let mut renderer: BlitterRenderer<'_, _, 32> =
                    BlitterRenderer::new(&mut blitter, framebuffer_surface(&mut surface_fb));
                root.borrow().draw(&mut renderer);
            }
            for y in 0..240u32 {
                for x in 0..240u32 {
                    display.set_pixel(x, y, surface_fb[(y * 240 + x) as usize]);
                }
            }
            if let Err(e) = display.flush().await {
                fail("flush", &e).await;
            }
            if !flushed_once {
                flushed_once = true;
                println!("SBX step=ui-ok");
            }
        }
    }
}

// ---------------------------------------------------------------------
// main
// ---------------------------------------------------------------------

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    // SRAM heap first — nothing may allocate before this line.
    esp_alloc::heap_allocator!(size: 96 * 1024);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    println!("SBX step=boot");

    let psram = esp_hal::psram::Psram::new(peripherals.PSRAM, Default::default());
    let (_, psram_bytes) = psram.raw_parts();
    if psram_bytes > 0 {
        esp_alloc::psram_allocator!(&psram);
    }
    println!("SBX psram_bytes={psram_bytes}");

    spawner.spawn(settings_task(peripherals.FLASH).unwrap());
    spawner.spawn(uart_task(peripherals.UART0, peripherals.GPIO43, peripherals.GPIO44).unwrap());
    spawner.spawn(
        sensor_task((
            peripherals.I2C0,
            peripherals.GPIO6,
            peripherals.GPIO7,
            peripherals.GPIO13,
            peripherals.GPIO5,
        ))
        .unwrap(),
    );
    spawner.spawn(battery_task(peripherals.ADC1, peripherals.GPIO1).unwrap());
    spawner.spawn(button_task(peripherals.GPIO0).unwrap());
    spawner.spawn(
        ui_task(
            (
                peripherals.SPI2,
                peripherals.DMA_CH0,
                peripherals.GPIO10,
                peripherals.GPIO11,
                peripherals.GPIO9,
                peripherals.GPIO8,
                peripherals.GPIO14,
                peripherals.GPIO2,
                peripherals.LEDC,
            ),
            psram_bytes,
        )
        .unwrap(),
    );

    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}
