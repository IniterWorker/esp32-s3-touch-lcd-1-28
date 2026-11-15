//! Embassy async HIL test for the qmi8658 driver (I2C, ESP32-S3-Touch-LCD-1.28).
//!
//! Polls samples on a 500 ms cadence with the async driver, configures
//! tap + any-motion with activity interrupts routed to INT1 (GPIO4),
//! and logs activity/tap events both on INT1 rising edges (esp-hal
//! async GPIO) and on STATUS1 polls. Tap or shake the board to see
//! `step=activity` / `step=tap` markers. INT2 (GPIO3) is unused.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Timer, with_timeout};
use esp_hal::{
    clock::CpuClock,
    gpio::{Input, InputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_println::println;
use qmi8658::{
    DeviceAddress, Qmi8658Async,
    config::{Config, InterruptConfig, MotionConfig, MotionEngines, TapConfig},
};

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

const MODE: &str = "async";

fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("QMI8658-HIL mode={MODE} step={step} result=FAIL err={err:?}");
    loop {
        core::hint::spin_loop();
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    println!("QMI8658-HIL mode={MODE} step=boot");

    // Shared I2C bus (CST816S touch + QMI8658): SDA=GPIO6, SCL=GPIO7.
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap_or_else(|e| fail("i2c", &e))
    .with_sda(peripherals.GPIO6)
    .with_scl(peripherals.GPIO7)
    .into_async();

    // IMU INT1 on GPIO4; the sensor drives it push-pull (idle low).
    let mut int1 = Input::new(
        peripherals.GPIO4,
        InputConfig::default().with_pull(Pull::Down),
    );

    // Secondary address on this board (SA0 low).
    let mut imu = Qmi8658Async::new_i2c(i2c, DeviceAddress::Secondary, Delay);
    Timer::after(Duration::from_millis(20)).await; // power-up settling

    imu.init(Config::default())
        .await
        .unwrap_or_else(|e| fail("init", &e));
    println!("QMI8658-HIL mode={MODE} step=init-ok");

    // Activity events (tap + motion) onto INT1; keep DRDY on INT2.
    imu.configure_interrupts(&InterruptConfig {
        activity_interrupt_on_int1: true,
        ..InterruptConfig::default()
    })
    .await
    .unwrap_or_else(|e| fail("interrupts", &e));

    match imu.configure_tap(&TapConfig::default(), true).await {
        Ok(()) => println!("QMI8658-HIL mode={MODE} step=tap-config ok=1"),
        Err(e) => println!("QMI8658-HIL mode={MODE} step=tap-config err={e:?}"),
    }
    match imu
        .configure_motion(
            &MotionConfig::default(),
            MotionEngines {
                any_motion: true,
                no_motion: false,
                significant_motion: false,
            },
        )
        .await
    {
        Ok(()) => println!("QMI8658-HIL mode={MODE} step=motion-config ok=1"),
        Err(e) => println!("QMI8658-HIL mode={MODE} step=motion-config err={e:?}"),
    }

    // Gyro data valid ~150 ms after enable (engines already restarted it).
    Timer::after(Duration::from_millis(250)).await;
    println!("QMI8658-HIL mode={MODE} step=ready note=tap-or-shake-board");

    let mut i: u32 = 0;
    loop {
        // Wait for an INT1 rising edge, but at most 500 ms so samples
        // keep flowing on a steady cadence.
        let edge = with_timeout(Duration::from_millis(500), int1.wait_for_rising_edge()).await;
        if edge.is_ok() {
            println!("QMI8658-HIL mode={MODE} step=int1-edge");
        }

        match imu.activity_status().await {
            Ok(a) if a.any() => {
                println!(
                    "QMI8658-HIL mode={MODE} step=activity sig={} no={} any={} pedo={} wom={} tap={}",
                    a.significant_motion,
                    a.no_motion,
                    a.any_motion,
                    a.pedometer_event,
                    a.wake_on_motion,
                    a.tap,
                );
                if a.tap {
                    match imu.tap_event().await {
                        Ok(Some(ev)) => println!(
                            "QMI8658-HIL mode={MODE} step=tap count={:?} axis={:?} negative={}",
                            ev.count, ev.axis, ev.negative_polarity
                        ),
                        Ok(None) => {}
                        Err(e) => println!("QMI8658-HIL mode={MODE} step=tap err={e:?}"),
                    }
                }
            }
            Ok(_) => {}
            Err(e) => println!("QMI8658-HIL mode={MODE} step=activity err={e:?}"),
        }

        match imu.read_sample().await {
            Ok(s) => println!(
                "QMI8658-HIL mode={MODE} step=sample i={i} accel_g=[{:.3},{:.3},{:.3}] gyro_dps=[{:.2},{:.2},{:.2}] temp_c={:.1}",
                s.accel_g[0], s.accel_g[1], s.accel_g[2], s.gyro_dps[0], s.gyro_dps[1],
                s.gyro_dps[2], s.temperature_c,
            ),
            Err(e) => println!("QMI8658-HIL mode={MODE} step=sample i={i} err={e:?}"),
        }
        i = i.wrapping_add(1);
    }
}
