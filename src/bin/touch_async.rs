//! Async (Embassy) HIL test for the cst816s-rs driver (CST816S touch, I2C).
//!
//! Same sequence and markers as `touch_blocking` (`CST816S-REF
//! mode=async ...`), plus a 1 s heartbeat task proving executor
//! liveness. Final loop awaits the touch INT pin (GPIO5) through the
//! driver's `wait_for_touch`, with a 5 s timeout fallback.
//! Interactive check: `just flash-touch-async`, then tap/slide and
//! watch `step=touch` coordinates and gesture names.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use cst816s::{
    Cst816sAsync,
    config::ReportMode,
    data::{ChipId, Gesture},
};
use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Instant, Timer, with_timeout};
use esp_hal::{
    clock::CpuClock,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::I2c,
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp32_s3_touch_lcd_1_28::board;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

const MODE: &str = "async";

async fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("CST816S-REF mode={MODE} step={step} result=FAIL err={err:?}");
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Executor-liveness proof: heartbeats must keep interleaving with the
/// touch sequence; `scripts/hil-touch.sh` requires >= 3 and >= 1
/// between `step=probe-ok` and `result=ALL-PASS`.
#[embassy_executor::task]
async fn heartbeat_task() {
    let start = Instant::now();
    loop {
        Timer::after(Duration::from_secs(1)).await;
        println!(
            "CST816S-REF mode={MODE} step=heartbeat uptime_s={}",
            start.elapsed().as_secs()
        );
    }
}

async fn assert_known_chip(chip: ChipId) {
    if let ChipId::Unknown(raw) = chip {
        println!("CST816S-REF mode={MODE} step=probe-unknown raw=0x{raw:02x}");
        fail("probe", &chip).await;
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

    spawner.spawn(heartbeat_task().unwrap());

    println!("CST816S-REF mode={MODE} step=boot cst816s=0.2.0");

    // Shared I2C bus (CST816S touch + QMI8658): SDA=GPIO6, SCL=GPIO7, 400 kHz.
    let i2c = match I2c::new(peripherals.I2C0, board::i2c_config()) {
        Ok(i2c) => i2c
            .with_sda(peripherals.GPIO6)
            .with_scl(peripherals.GPIO7)
            .into_async(),
        Err(e) => fail("i2c", &e).await,
    };

    let rst = Output::new(peripherals.GPIO13, Level::High, OutputConfig::default());
    let int = Input::new(
        peripherals.GPIO5,
        InputConfig::default().with_pull(Pull::Up),
    );

    let mut touch = Cst816sAsync::new(i2c, Delay)
        .with_reset_pin(rst)
        .with_interrupt_pin(int);

    let info = match touch.init().await {
        Ok(i) => i,
        Err(e) => fail("init", &e).await,
    };
    assert_known_chip(info.chip).await;
    println!(
        "CST816S-REF mode={MODE} step=probe-ok chip={:?} proj={} fw={}",
        info.chip, info.project_id, info.firmware_version
    );

    if let Err(e) = touch.set_irq_pulse_width(20).await {
        fail("config", &e).await;
    }
    match touch.irq_pulse_width().await {
        Ok(20) => {}
        Ok(v) => fail("config-irq-pulse-width", &v).await,
        Err(e) => fail("config", &e).await,
    }
    if let Err(e) = touch.set_auto_reset(0).await {
        fail("config", &e).await;
    }
    match touch.auto_reset().await {
        Ok(0) => {}
        Ok(v) => fail("config-auto-reset", &v).await,
        Err(e) => fail("config", &e).await,
    }
    if let Err(e) = touch.set_report_mode(ReportMode::PointAndGesture).await {
        fail("config", &e).await;
    }
    match touch.irq_ctl().await {
        Ok(c) if c.to_raw() == 0x71 => {}
        Ok(c) => fail("config-irq-ctl", &c.to_raw()).await,
        Err(e) => fail("config", &e).await,
    }
    // Same firmware auto-sleep behaviour as the blocking bin (proj105
    // sleeps + NACKs ~0.5 s after AutoSleepTime=2): keep the chip awake
    // so INT-driven reads stay live indefinitely.
    if let Err(e) = touch.set_disable_auto_sleep(true).await {
        fail("config", &e).await;
    }
    match touch.disable_auto_sleep().await {
        Ok(true) => {}
        Ok(v) => fail("config-disable-auto-sleep", &v).await,
        Err(e) => fail("config", &e).await,
    }
    // Enable continuous slide + double-click motion reporting and
    // verify the register. Hardware finding: this firmware (CST816T
    // proj105) never sets the gesture byte even with the mask on
    // (5000+ flick frames, all gesture=None) — it is a point firmware;
    // applications derive gestures from coordinate trajectories.
    let mm = cst816s::config::MotionMask {
        continuous_left_right: true,
        continuous_up_down: true,
        double_click: true,
    };
    if let Err(e) = touch.set_motion_mask(mm).await {
        fail("config", &e).await;
    }
    match touch.motion_mask().await {
        Ok(v) if v == mm => {}
        Ok(v) => fail("config-motion-mask", &v).await,
        Err(e) => fail("config", &e).await,
    }
    println!("CST816S-REF mode={MODE} step=config-ok");

    match touch.read_touch().await {
        Ok(r) if r.fingers == 0 && !r.pressed => {}
        Ok(r) => fail("idle", &r).await,
        Err(e) => fail("idle", &e).await,
    }
    println!("CST816S-REF mode={MODE} step=idle-ok");

    if let Err(e) = touch.enter_deep_sleep().await {
        fail("sleep", &e).await;
    }
    Timer::after(Duration::from_millis(200)).await;
    if let Err(e) = touch.wake().await {
        fail("wake", &e).await;
    }
    match touch.probe().await {
        Ok(i) => assert_known_chip(i.chip).await,
        Err(e) => fail("probe2", &e).await,
    }
    println!("CST816S-REF mode={MODE} step=sleep-wake-ok");

    // Dwell so at least one 1 s heartbeat lands before ALL-PASS
    // (executor-liveness evidence for the HIL script).
    Timer::after(Duration::from_millis(1200)).await;
    println!("CST816S-REF mode={MODE} result=ALL-PASS");

    // Manual/visual check only; automated HIL does not assert these lines.
    loop {
        match with_timeout(Duration::from_secs(5), touch.wait_for_touch()).await {
            Ok(Ok(r)) if r.pressed || r.gesture != Gesture::None => {
                println!(
                    "CST816S-REF mode={MODE} step=touch x={} y={} gesture={:?} event={:?}",
                    r.x, r.y, r.gesture, r.event
                );
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => println!("CST816S-REF mode={MODE} step=touch err={e:?}"),
            Err(_) => println!("CST816S-REF mode={MODE} step=touch-timeout"),
        }
    }
}
