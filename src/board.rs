//! Board support for the Waveshare ESP32-S3-Touch-LCD-1.28.
//!
//! Everything here is ESP32-S3-specific and only compiled with the `board`
//! feature. The pure drawing core lives in [`crate::test_card`].

use esp_hal::{spi::Mode, spi::master::Config, time::Rate};
use static_cell::StaticCell;

/// GPIO assignments of this board, carried over unchanged from the pre-rewrite
/// ESP-IDF firmware (see git tag `pre-rewrite-espidf`, `src/main.rs`).
///
/// The firmware bins take their pins from `peripherals.GPIOx` directly; these
/// constants are the documented pin map (and what the deferred touch/IMU bins
/// will build on).
#[allow(dead_code)]
pub mod pins {
    /// LCD SPI clock.
    pub const LCD_SCK: u8 = 10;
    /// LCD SPI MOSI (no MISO on this panel).
    pub const LCD_MOSI: u8 = 11;
    /// LCD SPI chip-select.
    pub const LCD_CS: u8 = 9;
    /// LCD data/command.
    pub const LCD_DC: u8 = 8;
    /// LCD reset.
    pub const LCD_RST: u8 = 14;
    /// LCD backlight (LEDC PWM in the reference-test bins).
    pub const LCD_BL: u8 = 2;
    /// Shared I2C data (CST816S touch + QMI8658 IMU).
    pub const I2C_SDA: u8 = 6;
    /// Shared I2C clock (CST816S touch + QMI8658 IMU).
    pub const I2C_SCL: u8 = 7;
    /// CST816S touch interrupt.
    pub const TOUCH_INT: u8 = 5;
    /// CST816S touch reset.
    pub const TOUCH_RST: u8 = 13;
    /// QMI8658 IMU interrupt 1.
    pub const IMU_INT1: u8 = 4;
    /// QMI8658 IMU interrupt 2.
    pub const IMU_INT2: u8 = 3;
}

/// Framebuffer storage: 240 * 240 `u16` = 115 200 bytes. That must never live
/// on the stack, so it is a single static handed to
/// `into_buffered_graphics_with_buffer`. Only one firmware bin runs per flash,
/// so one shared static is correct.
pub static FRAMEBUFFER: StaticCell<[u16; 240 * 240]> = StaticCell::new();

/// Claim the static framebuffer (zero-initialized). Panics on a second call;
/// each bin calls this exactly once.
pub fn framebuffer() -> &'static mut [u16; 240 * 240] {
    FRAMEBUFFER.init([0; 240 * 240])
}

/// The LCD SPI configuration both bins share: 40 MHz, mode 0 — the settings
/// the pre-rewrite firmware used.
pub fn lcd_spi_config() -> Config {
    Config::default()
        .with_frequency(Rate::from_mhz(40))
        .with_mode(Mode::_0)
}
