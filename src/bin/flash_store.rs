//! Flash settings-store reference test (16 MB W25Q128, `settings` partition).
//!
//! Sequence: boot → open store (partition-table label lookup) → load →
//! save with `backlight_pct` toggled 100↔42 → re-open scan equivalent
//! (reload) → assert round-trip → `FLASH-REF step=write-read-ok`.
//! `scripts/hil.sh flash_store` asserts the markers; running it twice
//! proves the sequence number increments across boots.
//!
//! NOTE: this bin mutates the sandbox settings (backlight toggles each
//! run). That is acceptable — it is a store exerciser, not a viewer.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embedded_hal::delay::DelayNs;
use esp_hal::{clock::CpuClock, delay::Delay};
use esp_println::println;
use esp_storage::FlashStorage;
use esp32_s3_touch_lcd_1_28::settings::SettingsStore;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

fn fail(step: &str, err: &dyn core::fmt::Debug) -> ! {
    println!("FLASH-REF step={step} result=FAIL err={err:?}");
    loop {
        Delay::new().delay_ms(1000);
    }
}

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    println!("FLASH-REF step=boot");

    let flash = FlashStorage::new(peripherals.FLASH);
    let mut store = match SettingsStore::open(flash) {
        Some(s) => s,
        None => fail("probe", &"partition-table-unreadable"),
    };
    println!(
        "FLASH-REF step=probe-ok stored={}",
        u8::from(store.has_stored())
    );

    let (seq0, mut settings) = store.load();
    println!(
        "FLASH-REF step=loaded seq={seq0} backlight={}",
        settings.backlight_pct
    );

    settings.backlight_pct = if settings.backlight_pct == 100 {
        42
    } else {
        100
    };
    let seq1 = store.save(&settings).unwrap_or_else(|e| fail("save", &e));

    let (seq2, loaded) = store.load();
    if seq2 != seq1 || loaded != settings || !(seq1 == seq0 + 1 || (seq0 == 0 && seq1 == 1)) {
        fail("verify", &(seq0, seq1, seq2, loaded.backlight_pct));
    }
    println!("FLASH-REF step=write-read-ok seq={seq2}");

    loop {
        Delay::new().delay_ms(1000);
    }
}
