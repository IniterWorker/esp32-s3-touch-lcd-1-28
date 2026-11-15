//! Blocking HIL test for the qmi8658 driver (I2C, ESP32-S3-Touch-LCD-1.28).
//!
//! Phases: init + 10 Hz sample polling → accel/gyro self-tests → FIFO
//! drain → gyro calibration-on-demand → pedometer attempt (records
//! whether the fitted silicon variant implements it) → sample polling
//! forever. Markers are `QMI8658-HIL mode=blocking ...` on UART.

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
    clock::CpuClock, delay::Delay, i2c::master::{Config as I2cConfig, I2c}, time::Rate,
};
use esp_println::println;
use qmi8658::{
    DeviceAddress, Qmi8658,
    config::{Config, FifoConfig, PedometerConfig},
    data::FifoFrames,
};
use static_cell::StaticCell;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

const MODE: &str = "blocking";

/// FIFO drain buffer: 128 samples × 12 bytes, must not live on the stack.
static FIFO_BUF: StaticCell<[u8; 1536]> = StaticCell::new();

fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("QMI8658-HIL mode={MODE} step={step} result=FAIL err={err:?}");
    loop {
        Delay::new().delay_ms(1000);
    }
}

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    println!("QMI8658-HIL mode={MODE} step=boot");

    // Shared I2C bus (CST816S touch + QMI8658): SDA=GPIO6, SCL=GPIO7.
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap_or_else(|e| fail("i2c", &e))
    .with_sda(peripherals.GPIO6)
    .with_scl(peripherals.GPIO7);

    // This board's QMI8658 sits at the secondary address (SA0 low), as
    // in the pre-rewrite firmware (`new_secondary_address`).
    let mut imu = Qmi8658::new_i2c(i2c, DeviceAddress::Secondary, Delay::new());
    let mut delay = Delay::new();
    delay.delay_ms(20); // sensor power-up settling (>= 15 ms)

    match imu.device_id() {
        Ok(id) => println!(
            "QMI8658-HIL mode={MODE} step=device-id who_am_i=0x{:02x} revision=0x{:02x}",
            id.who_am_i, id.revision
        ),
        Err(e) => println!("QMI8658-HIL mode={MODE} step=device-id-pre-init err={e:?}"),
    }

    imu.init(Config::default())
        .unwrap_or_else(|e| fail("init", &e));
    println!("QMI8658-HIL mode={MODE} step=init-ok");

    // Gyro data is valid ~150 ms after enable.
    delay.delay_ms(250);

    // Phase 1: 10 Hz samples for ~2 s. Board flat and still expect
    // |accel| ~1 g, gyro within a few dps, temperature 15-40 °C.
    for i in 0..20 {
        match imu.read_sample() {
            Ok(s) => println!(
                "QMI8658-HIL mode={MODE} step=sample i={i} ts={} accel_g=[{:.3},{:.3},{:.3}] gyro_dps=[{:.2},{:.2},{:.2}] temp_c={:.1}",
                s.timestamp,
                s.accel_g[0],
                s.accel_g[1],
                s.accel_g[2],
                s.gyro_dps[0],
                s.gyro_dps[1],
                s.gyro_dps[2],
                s.temperature_c,
            ),
            Err(e) => println!("QMI8658-HIL mode={MODE} step=sample i={i} err={e:?}"),
        }
        delay.delay_ms(100);
    }

    // Phase 2: self-tests (datasheet thresholds: accel >200 mg, gyro >300 dps).
    match imu.self_test_accel() {
        Ok(r) => println!(
            "QMI8658-HIL mode={MODE} step=selftest-accel passed={} delta_mg=[{:.0},{:.0},{:.0}]",
            r.passed, r.delta[0], r.delta[1], r.delta[2]
        ),
        Err(e) => println!("QMI8658-HIL mode={MODE} step=selftest-accel err={e:?}"),
    }
    match imu.self_test_gyro() {
        Ok(r) => println!(
            "QMI8658-HIL mode={MODE} step=selftest-gyro passed={} delta_dps=[{:.0},{:.0},{:.0}]",
            r.passed, r.delta[0], r.delta[1], r.delta[2]
        ),
        Err(e) => println!("QMI8658-HIL mode={MODE} step=selftest-gyro err={e:?}"),
    }

    // Sensors were stopped by the self-tests' register save/restore only
    // if they were on before; re-apply the running config explicitly.
    imu.apply_config(Config::default())
        .unwrap_or_else(|e| fail("reapply", &e));
    delay.delay_ms(250);

    // Phase 3: FIFO stream for 1 s, then drain.
    imu.configure_fifo(FifoConfig::default())
        .unwrap_or_else(|e| fail("fifo-config", &e));
    delay.delay_ms(1000);
    match imu.fifo_status() {
        Ok(st) => println!(
            "QMI8658-HIL mode={MODE} step=fifo-status bytes={} full={} wtm={} overflow={}",
            st.bytes_available, st.full, st.watermark_reached, st.overflow
        ),
        Err(e) => println!("QMI8658-HIL mode={MODE} step=fifo-status err={e:?}"),
    }
    let buf = FIFO_BUF.init([0u8; 1536]);
    match imu.read_fifo(buf) {
        Ok(n) => {
            let frames = FifoFrames::new(&buf[..n], imu.fifo_format()).count();
            println!("QMI8658-HIL mode={MODE} step=fifo-read bytes={n} frames={frames}");
        }
        Err(e) => println!("QMI8658-HIL mode={MODE} step=fifo-read err={e:?}"),
    }

    // Phase 4: calibration-on-demand. Keep the board still for ~2 s.
    println!("QMI8658-HIL mode={MODE} step=cod-start note=keep-board-still");
    match imu.calibrate_gyro_on_demand() {
        Ok(gains) => {
            println!(
                "QMI8658-HIL mode={MODE} step=cod-ok gains={:02x?}",
                gains.0
            );
            match imu.apply_gyro_gains(&gains) {
                Ok(()) => println!("QMI8658-HIL mode={MODE} step=cod-apply-gains ok=1"),
                Err(e) => println!("QMI8658-HIL mode={MODE} step=cod-apply-gains err={e:?}"),
            }
        }
        Err(e) => {
            let status = imu.cod_status().map(|s| s.to_bits()).unwrap_or(0xFF);
            println!(
                "QMI8658-HIL mode={MODE} step=cod-fail err={e:?} cod_status=0x{status:02x}"
            );
        }
    }
    imu.apply_config(Config::default())
        .unwrap_or_else(|e| fail("reapply2", &e));
    delay.delay_ms(250);

    // Phase 5: pedometer is a C-variant feature; success vs
    // CommandTimeout tells us which silicon is fitted.
    match imu.configure_pedometer(&PedometerConfig::default(), true) {
        Ok(()) => {
            println!("QMI8658-HIL mode={MODE} step=pedometer supported=1");
            match imu.step_count() {
                Ok(n) => println!("QMI8658-HIL mode={MODE} step=step-count value={n}"),
                Err(e) => println!("QMI8658-HIL mode={MODE} step=step-count err={e:?}"),
            }
        }
        Err(e) => println!("QMI8658-HIL mode={MODE} step=pedometer supported=0 err={e:?}"),
    }

    println!("QMI8658-HIL mode={MODE} step=phases-done");
    loop {
        match imu.read_sample() {
            Ok(s) => println!(
                "QMI8658-HIL mode={MODE} step=sample accel_g=[{:.3},{:.3},{:.3}] gyro_dps=[{:.2},{:.2},{:.2}] temp_c={:.1}",
                s.accel_g[0], s.accel_g[1], s.accel_g[2], s.gyro_dps[0], s.gyro_dps[1],
                s.gyro_dps[2], s.temperature_c,
            ),
            Err(e) => println!("QMI8658-HIL mode={MODE} step=sample err={e:?}"),
        }
        delay.delay_ms(1000);
    }
}
