//! Battery voltage reference test (BAT_ADC on GPIO1, ADC1).
//!
//! Single blocking bin deliberately: ADC one-shot reads are synchronous
//! register pokes, an "async variant" would be the same code in an
//! async fn. Prints `BAT-REF mv=<n> pct=<n> raw=<n>` once per second
//! (8-read average); conversion lives in `battery::{battery_mv,
//! battery_pct}` (200k/100k divider, ×3).
//!
//! Note: on USB power with no battery attached the node floats toward
//! the charge voltage — the HIL asserts line *format* only; plausibility
//! is checked by a human with a battery attached (vs multimeter).
//! Charge state is not readable in software: the ETA6096 STAT output
//! drives an LED only, no GPIO (Rev3 schematic).

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
    analog::adc::{Adc, AdcConfig, Attenuation},
    clock::CpuClock,
    delay::Delay,
};
use esp_println::println;
use esp32_s3_touch_lcd_1_28::battery::{battery_mv, battery_pct};

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    println!("BAT-REF step=boot");

    // GPIO1 = ADC1 channel 0; 11 dB attenuation for full 0..3.3 V range
    // at the pin (0..9.9 V referred through the divider). Raw codes,
    // no calibration scheme: battery_mv() assumes raw.
    let mut adc_config = AdcConfig::new();
    let mut pin = adc_config.enable_pin(peripherals.GPIO1, Attenuation::_11dB);
    let mut adc = Adc::new(peripherals.ADC1, adc_config);

    let mut delay = Delay::new();
    loop {
        let mut sum: u32 = 0;
        for _ in 0..8 {
            let raw: u16 = nb::block!(adc.read_oneshot(&mut pin)).unwrap_or(0);
            sum += u32::from(raw);
        }
        let raw = (sum / 8) as u16;
        let mv = battery_mv(raw);
        println!("BAT-REF mv={mv} pct={} raw={raw}", battery_pct(mv));
        delay.delay_ms(1000);
    }
}
