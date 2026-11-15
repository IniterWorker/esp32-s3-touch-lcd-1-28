//! Board support for the Waveshare ESP32-S3-Touch-LCD-1.28.
//!
//! Everything here is ESP32-S3-specific and only compiled with the `board`
//! feature. The pure drawing core lives in [`crate::test_card`].

use core::convert::Infallible;

use esp_hal::{
    gpio::DriveMode,
    i2c::master::Config as I2cConfig,
    ledc::{
        LSGlobalClkSource, Ledc, LowSpeed,
        channel::{self, Channel, ChannelIFace},
        timer::{self, Timer, TimerIFace},
    },
    peripherals::{GPIO2, LEDC},
    spi::{Mode, master::Config},
    time::Rate,
};
use static_cell::StaticCell;

/// GPIO assignments of this board (Waveshare pinout table + Rev3
/// schematic; display/I2C pins also carried over from the pre-rewrite
/// ESP-IDF firmware, git tag `pre-rewrite-espidf`).
///
/// The firmware bins take their pins from `peripherals.GPIOx` directly;
/// these constants are the documented pin map.
#[allow(dead_code)]
pub mod pins {
    /// LCD SPI clock.
    pub const LCD_SCK: u8 = 10;
    /// LCD SPI MOSI.
    pub const LCD_MOSI: u8 = 11;
    /// LCD SPI MISO — wired on this board; the reference bins never read
    /// the panel, so they don't connect it.
    pub const LCD_MISO: u8 = 12;
    /// LCD SPI chip-select.
    pub const LCD_CS: u8 = 9;
    /// LCD data/command.
    pub const LCD_DC: u8 = 8;
    /// LCD reset.
    pub const LCD_RST: u8 = 14;
    /// LCD backlight MOSFET gate (LEDC PWM).
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
    /// Battery voltage sense (ADC1, 200k/100k divider → ×3).
    pub const BAT_ADC: u8 = 1;
    /// BOOT button (strapping pin; usable as input after boot).
    pub const BOOT_BTN: u8 = 0;
    /// UART0 TX to the CH343P USB-UART bridge.
    pub const UART_TX: u8 = 43;
    /// UART0 RX from the CH343P USB-UART bridge.
    pub const UART_RX: u8 = 44;
    /// Native USB D−. The schematic places 0 Ω links between the ESP
    /// USB pins and the CH343-side Type-C lines — native USB usability
    /// alongside the bridge is UNVERIFIED; do not build on it.
    pub const USB_DM: u8 = 19;
    /// Native USB D+. See [`USB_DM`].
    pub const USB_DP: u8 = 20;
    /// GPIOs broken out on the SH1.0 header (plus VSYS/GND). GPIO33 is
    /// free on this R2 (quad PSRAM) SoC; on octal-PSRAM variants it
    /// would be a PSRAM data line — usable here, with that note.
    pub const SH1_0: [u8; 6] = [15, 16, 17, 18, 21, 33];
}

/// Framebuffer storage: 240 * 240 `u16` = 115 200 bytes. That must never live
/// on the stack, so it is a single static handed to
/// `into_buffered_graphics_with_buffer`. Only one firmware bin runs per flash,
/// so one shared static is correct.
pub static FRAMEBUFFER: StaticCell<[u16; 240 * 240]> = StaticCell::new();

/// Claim the static framebuffer (zero-initialized). Panics on a second call;
/// each bin calls this exactly once.
///
/// Zeroed in place via `uninit`: passing `[0; 57600]` by value to
/// `StaticCell::init` materializes a 115 KB stack temporary, which
/// tripped the stack guard in the sandbox bin.
pub fn framebuffer() -> &'static mut [u16; 240 * 240] {
    let slot = FRAMEBUFFER.uninit();
    // SAFETY: `uninit` claimed the cell exclusively; every element is
    // written before `assume_init_mut`, and 0x00 is a valid u16.
    unsafe {
        slot.as_mut_ptr().cast::<u16>().write_bytes(0, 240 * 240);
        slot.assume_init_mut()
    }
}

/// The LCD SPI configuration every bin shares: 40 MHz, mode 0 — the settings
/// the pre-rewrite firmware used.
pub fn lcd_spi_config() -> Config {
    Config::default()
        .with_frequency(Rate::from_mhz(40))
        .with_mode(Mode::_0)
}

/// The shared I2C configuration: 400 kHz fast mode (CST816S + QMI8658
/// both support it).
pub fn i2c_config() -> I2cConfig {
    I2cConfig::default().with_frequency(Rate::from_khz(400))
}

/// Board bring-up failures, unified so bins can `fail(.., &e)` them.
#[derive(Debug)]
pub enum InitError {
    /// LEDC timer configuration failed.
    LedcTimer(timer::Error),
    /// LEDC channel configuration (or duty set) failed.
    LedcChannel(channel::Error),
    /// SPI bus construction failed.
    Spi(esp_hal::spi::Error),
    /// Chip-select output pin failed (infallible in practice).
    Cs(Infallible),
}

impl From<timer::Error> for InitError {
    fn from(e: timer::Error) -> Self {
        Self::LedcTimer(e)
    }
}

impl From<channel::Error> for InitError {
    fn from(e: channel::Error) -> Self {
        Self::LedcChannel(e)
    }
}

/// Backlight LEDC timer storage. The LEDC channel borrows its timer,
/// so the timer must outlive every caller frame: it lives here for
/// `'static`. One backlight per board, one claim per boot.
static BACKLIGHT_TIMER: StaticCell<Timer<'static, LowSpeed>> = StaticCell::new();

/// Backlight LEDC PWM on GPIO2: 5 kHz, 8-bit, starting at 0 % (off
/// until the panel is initialized). PWM because the GC9A01's own
/// brightness register only drives its LEDPWM pin, which is not wired
/// to this board's backlight circuit (verified on hardware).
///
/// Panics on a second call (timer static already claimed); each bin
/// calls this exactly once.
///
/// # Errors
///
/// [`InitError::LedcTimer`] / [`InitError::LedcChannel`] on LEDC
/// configuration failure.
pub fn backlight_pwm(
    ledc_periph: LEDC<'static>,
    pin: GPIO2<'static>,
) -> Result<Channel<'static, LowSpeed>, InitError> {
    let mut ledc = Ledc::new(ledc_periph);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);
    let mut timer = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    timer.configure(timer::config::Config {
        duty: timer::config::Duty::Duty8Bit,
        clock_source: timer::LSClockSource::APBClk,
        frequency: Rate::from_khz(5),
    })?;
    let timer = BACKLIGHT_TIMER.init(timer);
    let mut channel = ledc.channel(channel::Number::Channel0, pin);
    channel.configure(channel::config::Config {
        timer,
        duty_pct: 0,
        drive_mode: DriveMode::PushPull,
    })?;
    Ok(channel)
}

/// Shared async I2C bus: the one I2C-sharing pattern for this repo.
///
/// Touch and IMU live on the same physical bus, and both async drivers
/// take an owned bus object. `SharedI2c` is a `Copy` handle to a
/// mutex-guarded bus; each driver gets its own handle, and every
/// transaction locks the bus for its duration.
pub mod shared_i2c {
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
    use embedded_hal_async::i2c::{ErrorType, I2c, Operation};

    /// Mutex type guarding the shared bus. Put the bus in a `static`
    /// (e.g. via `static_cell`) and hand out [`SharedI2c`] handles.
    pub type SharedI2cBus<I2C> = Mutex<CriticalSectionRawMutex, I2C>;

    /// A copyable async I2C device backed by a shared, locked bus.
    pub struct SharedI2c<'a, I2C>(pub &'a SharedI2cBus<I2C>);

    impl<I2C> Clone for SharedI2c<'_, I2C> {
        fn clone(&self) -> Self {
            *self
        }
    }
    impl<I2C> Copy for SharedI2c<'_, I2C> {}

    impl<I2C: I2c> ErrorType for SharedI2c<'_, I2C> {
        type Error = I2C::Error;
    }

    impl<I2C: I2c> I2c for SharedI2c<'_, I2C> {
        async fn transaction(
            &mut self,
            address: u8,
            operations: &mut [Operation<'_>],
        ) -> Result<(), Self::Error> {
            let mut bus = self.0.lock().await;
            bus.transaction(address, operations).await
        }
    }
}
