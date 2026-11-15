//! Blocking HIL test for the cst816s-rs driver (CST816S touch, I2C).
//!
//! Sequence: boot → init/probe → config write+readback → idle frame →
//! deep-sleep/wake → ALL-PASS → poll touch forever (manual check).
//! Markers: `CST816S-REF mode=blocking ...` on UART; `scripts/hil-touch.sh`
//! asserts them. Interactive check: touch the panel and watch
//! `step=touch` coordinates track the finger (0–239).

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use cst816s::{
    Cst816s,
    config::ReportMode,
    data::{ChipId, Gesture},
};
use embedded_hal::delay::DelayNs;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::I2c,
};
use esp_println::println;
use esp32_s3_touch_lcd_1_28::board;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

const MODE: &str = "blocking";

fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("CST816S-REF mode={MODE} step={step} result=FAIL err={err:?}");
    loop {
        Delay::new().delay_ms(1000);
    }
}

fn assert_known_chip(chip: ChipId) {
    if let ChipId::Unknown(raw) = chip {
        println!("CST816S-REF mode={MODE} step=probe-unknown raw=0x{raw:02x}");
        fail("probe", &chip);
    }
}

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    println!("CST816S-REF mode={MODE} step=boot cst816s=0.2.0");

    // Shared I2C bus (CST816S touch + QMI8658): SDA=GPIO6, SCL=GPIO7, 400 kHz.
    let i2c = I2c::new(peripherals.I2C0, board::i2c_config())
        .unwrap_or_else(|e| fail("i2c", &e))
        .with_sda(peripherals.GPIO6)
        .with_scl(peripherals.GPIO7);

    let rst = Output::new(peripherals.GPIO13, Level::High, OutputConfig::default());
    let int = Input::new(
        peripherals.GPIO5,
        InputConfig::default().with_pull(Pull::Up),
    );

    let mut touch = Cst816s::new(i2c, Delay::new())
        .with_reset_pin(rst)
        .with_interrupt_pin(int);
    let mut delay = Delay::new();

    let info = touch.init().unwrap_or_else(|e| fail("init", &e));
    assert_known_chip(info.chip);
    println!(
        "CST816S-REF mode={MODE} step=probe-ok chip={:?} proj={} fw={}",
        info.chip, info.project_id, info.firmware_version
    );

    // Config write + readback. auto_reset=0: auto-reset kills live points
    // (InfiniTime fleet lesson).
    touch
        .set_irq_pulse_width(20)
        .unwrap_or_else(|e| fail("config", &e));
    let pw = touch
        .irq_pulse_width()
        .unwrap_or_else(|e| fail("config", &e));
    if pw != 20 {
        fail("config-irq-pulse-width", &pw);
    }
    touch
        .set_auto_reset(0)
        .unwrap_or_else(|e| fail("config", &e));
    let ar = touch.auto_reset().unwrap_or_else(|e| fail("config", &e));
    if ar != 0 {
        fail("config-auto-reset", &ar);
    }
    touch
        .set_report_mode(ReportMode::PointAndGesture)
        .unwrap_or_else(|e| fail("config", &e));
    let ctl = touch.irq_ctl().unwrap_or_else(|e| fail("config", &e));
    if ctl.to_raw() != 0x71 {
        fail("config-irq-ctl", &ctl.to_raw());
    }
    // This module's firmware (CST816T, project 105) enters bus-NACKing
    // sleep ~0.5 s after AutoSleepTime=2 takes effect (measured on
    // hardware: ~42 good 10 ms polls, then Address NACK; unit is not
    // the documented seconds here). A polling loop must therefore
    // disable auto-sleep; wake() re-applies it via the stored config.
    touch
        .set_disable_auto_sleep(true)
        .unwrap_or_else(|e| fail("config", &e));
    let das = touch
        .disable_auto_sleep()
        .unwrap_or_else(|e| fail("config", &e));
    if !das {
        fail("config-disable-auto-sleep", &das);
    }
    // Enable motion reporting + verify readback (see touch_async:
    // proj105 never sets the gesture byte; point firmware).
    let mm = cst816s::config::MotionMask {
        continuous_left_right: true,
        continuous_up_down: true,
        double_click: true,
    };
    touch
        .set_motion_mask(mm)
        .unwrap_or_else(|e| fail("config", &e));
    let mmr = touch.motion_mask().unwrap_or_else(|e| fail("config", &e));
    if mmr != mm {
        fail("config-motion-mask", &mmr);
    }
    println!("CST816S-REF mode={MODE} step=config-ok");

    let idle = touch.read_touch().unwrap_or_else(|e| fail("idle", &e));
    if idle.fingers != 0 || idle.pressed {
        fail("idle", &idle);
    }
    println!("CST816S-REF mode={MODE} step=idle-ok");

    touch
        .enter_deep_sleep()
        .unwrap_or_else(|e| fail("sleep", &e));
    delay.delay_ms(200);
    touch.wake().unwrap_or_else(|e| fail("wake", &e));
    let info2 = touch.probe().unwrap_or_else(|e| fail("probe2", &e));
    assert_known_chip(info2.chip);
    println!("CST816S-REF mode={MODE} step=sleep-wake-ok");

    println!("CST816S-REF mode={MODE} result=ALL-PASS");

    // Manual/visual check only; automated HIL does not assert these lines.
    // Bus errors print once per streak (a sleeping chip NACKs); recovery
    // (touch wake / reset) resumes touch lines silently.
    let mut in_err_streak = false;
    loop {
        match touch.read_touch() {
            Ok(r) => {
                in_err_streak = false;
                if r.pressed || r.gesture != Gesture::None {
                    println!(
                        "CST816S-REF mode={MODE} step=touch x={} y={} gesture={:?} event={:?}",
                        r.x, r.y, r.gesture, r.event
                    );
                }
            }
            Err(e) => {
                if !in_err_streak {
                    in_err_streak = true;
                    println!("CST816S-REF mode={MODE} step=touch err={e:?}");
                }
            }
        }
        delay.delay_ms(10);
    }
}
