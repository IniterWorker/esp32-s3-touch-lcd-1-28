//! Display backend plumbing: view the GC9A01 framebuffer as an rlvgl
//! `Surface`.
//!
//! Byte order (verified by source inspection + host test below):
//! `CpuBlitter` writes Rgb565 pixels as little-endian u16 into the
//! surface buffer. The GC9A01 buffered-graphics framebuffer stores
//! semantic u16 RGB565 values (0xF800 = red) and byte-swaps to BE on
//! flush. Xtensa is little-endian, so the u16 slice and the surface
//! byte slice over the same memory agree — no conversion on flush.

use rlvgl_platform::blit::{PixelFmt, Surface};

/// Panel width/height (square).
pub const PANEL: u32 = 240;

/// Build a full-panel Rgb565 surface borrowing the framebuffer bytes.
/// `fb` must hold exactly `PANEL * PANEL` pixels.
pub fn framebuffer_surface(fb: &mut [u16]) -> Surface<'_> {
    debug_assert_eq!(fb.len(), (PANEL * PANEL) as usize);
    // same allocation, len*2 bytes, u8 has no alignment requirement.
    // Endianness agreement argued in the module docs.
    let bytes =
        unsafe { core::slice::from_raw_parts_mut(fb.as_mut_ptr().cast::<u8>(), fb.len() * 2) };
    Surface::new(bytes, (PANEL * 2) as usize, PixelFmt::Rgb565, PANEL, PANEL)
}
