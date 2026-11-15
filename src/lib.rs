//! GC9A01 0.5 reference test for the Waveshare ESP32-S3-Touch-LCD-1.28.
//!
//! The library is split in two:
//!
//! - [`test_card`] — pure drawing core (no ESP dependencies), host-testable
//!   with `cargo test --lib --no-default-features`.
//! - [`board`] — ESP32-S3 pin map, framebuffer, SPI preset (feature `board`).
//!
//! The firmware bins (`blocking`, `embassy_async`) run the identical
//! reference-test sequence against both gc9a01 driver flavors.

#![no_std]

pub mod test_card;

#[cfg(feature = "board")]
pub mod board;
