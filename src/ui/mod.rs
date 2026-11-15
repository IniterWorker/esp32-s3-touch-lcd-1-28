//! Sandbox UI on rlvgl: state, pages, input conversion, and the
//! framebuffer display backend.
//!
//! Feature `ui` only — no ESP deps in this module tree, so host tests
//! render pages off-screen and assert pixels (notably the Rgb565
//! byte-order contract with the GC9A01 framebuffer).

pub mod app;
pub mod display_backend;
pub mod input;
pub mod pages;
pub mod state;

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicPtr, Ordering};

use static_cell::StaticCell;

use rlvgl_core::WidgetNode;
use rlvgl_core::bitmap_font::{BitmapFont, FONT_6X10};
use rlvgl_core::style::Style;
use rlvgl_core::widget::{Color, Rect, Widget};
use rlvgl_widgets::label::Label;

/// Queue of widget-originated `key=value` actions, drained by the app
/// into [`state::AppState::apply`] after each event/tick. This is how
/// widget callbacks (which only receive `&mut Self`) reach app state.
pub type Actions = Rc<RefCell<Vec<(&'static str, String)>>>;

/// Fresh empty action queue.
pub fn new_actions() -> Actions {
    Rc::new(RefCell::new(Vec::new()))
}

/// Push one action onto the queue.
pub fn push_action(actions: &Actions, key: &'static str, value: impl Into<String>) {
    actions.borrow_mut().push((key, value.into()));
}

/// Wrap a concrete widget: returns the typed handle (for tick-time
/// updates) and the type-erased tree node sharing the same cell.
pub fn wrap<W: Widget + 'static>(w: W) -> (Rc<RefCell<W>>, WidgetNode) {
    let cell = Rc::new(RefCell::new(w));
    let node = WidgetNode::new(cell.clone());
    (cell, node)
}

/// The built-in `FONT_6X10` at 1× scale (6×10 px glyphs, 7 px
/// advance). rlvgl's default instance is scale 2 (12×20), which
/// overflows this panel's 12 px label rows and 168 px safe width;
/// titles keep the 2× default, body text uses this. Stored in a
/// `StaticCell` — fonts are process-lifetime assets in rlvgl's model.
pub fn small_font() -> &'static BitmapFont {
    static SMALL: StaticCell<BitmapFont> = StaticCell::new();
    static PTR: AtomicPtr<BitmapFont> = AtomicPtr::new(core::ptr::null_mut());
    let p = PTR.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: target lives in SMALL (static storage), never mutated.
        return unsafe { &*p };
    }
    // First caller claims the cell; racing callers (host tests) get
    // None and fall through to the published pointer.
    if let Some(f) = SMALL.try_init_with(|| BitmapFont {
        glyph_width: 6,
        glyph_height: 10,
        scale: 1,
        data: FONT_6X10.data,
    }) {
        PTR.store(f, Ordering::Release);
        return f;
    }
    // SAFETY: the winning caller stores before any loser can observe
    // `try_init_with` returning None; spin is not needed in practice
    // here (single ui_task on device), but wait for publication.
    loop {
        let p = PTR.load(Ordering::Acquire);
        if !p.is_null() {
            return unsafe { &*p };
        }
        core::hint::spin_loop();
    }
}

/// A body-text label (1× font): transparent background, given color.
/// Bounds height should be ≥ 10 (glyph height); pages use 12.
pub fn styled_label(
    text: impl Into<String>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color,
) -> Label {
    let mut l = make_label(text, x, y, w, h, color);
    l.set_font(small_font());
    l
}

/// A title label (rlvgl default 2× font, 12×20 glyphs). Bounds
/// height must be ≥ 20 or glyph bottoms clip.
pub fn styled_title(text: impl Into<String>, x: i32, y: i32, w: i32, color: Color) -> Label {
    make_label(text, x, y, w, 22, color)
}

fn make_label(text: impl Into<String>, x: i32, y: i32, w: i32, h: i32, color: Color) -> Label {
    let mut l = Label::new(
        text,
        Rect {
            x,
            y,
            width: w,
            height: h,
        },
    );
    // Transparent bg via bg_color alpha ONLY. Style::alpha must stay
    // 255: Label multiplies the *text* color by it, so alpha=0 makes
    // text invisible (the on-device "black screen, no UI" bug).
    l.style = Style {
        bg_color: Color(0, 0, 0, 0),
        ..Style::default()
    };
    #[allow(deprecated)]
    {
        l.text_color = color;
    }
    l.set_text_color(color);
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::display_backend::framebuffer_surface;
    use rlvgl_platform::CpuBlitter;
    use rlvgl_platform::blit::BlitterRenderer;

    /// End-to-end byte-order proof without a panel: render the hello
    /// page into a GC9A01-shaped u16 framebuffer and check the
    /// swatch centers hold the exact RGB565 values the reference
    /// bins proved on hardware (0xF800 red, 0x07E0 green, 0x001F blue).
    #[test]
    fn hello_swatches_land_as_gc9a01_rgb565() {
        let (root, _handles) = pages::build_hello(&state::AppState::default(), &new_actions());
        let mut fb = alloc::vec![0u16; 240 * 240];
        let mut blitter = CpuBlitter;
        let mut renderer: BlitterRenderer<'_, _, 32> =
            BlitterRenderer::new(&mut blitter, framebuffer_surface(&mut fb));
        root.draw(&mut renderer);
        let px = |x: usize, y: usize| fb[y * 240 + x];
        assert_eq!(px(72, 116), 0xF800, "RED swatch");
        assert_eq!(px(120, 116), 0x07E0, "GREEN swatch");
        assert_eq!(px(168, 116), 0x001F, "BLUE swatch");
        assert_ne!(px(120, 44), px(72, 116), "title text area differs from red");
    }

    /// Every page builds, refreshes with live-looking state, syncs
    /// controls, and draws non-blank output (catches canvas/index
    /// panics off-device).
    #[test]
    fn all_pages_render() {
        let mut state = state::AppState::default();
        state.battery_mv = 3900;
        state.imu = Some(state::ImuSample {
            ax_mg: 10,
            ay_mg: -20,
            az_mg: 1000,
            gx_mdps: 1000,
            gy_mdps: -2000,
            gz_mdps: 3000,
            temp_dc: 250,
        });
        state.heading_deg = 137;
        state.last_touch = Some((100, 120));
        let actions = new_actions();
        for page in 0..state::PAGE_COUNT {
            state.page = page;
            let (root, handles) = pages::build_page(page, &state, &actions);
            pages::refresh(&handles, &state, 7);
            let mut st = state::AppState::default();
            pages::sync_controls(&handles, &mut st);
            let mut fb = alloc::vec![0u16; 240 * 240];
            let mut blitter = CpuBlitter;
            let mut renderer: BlitterRenderer<'_, _, 32> =
                BlitterRenderer::new(&mut blitter, framebuffer_surface(&mut fb));
            root.draw(&mut renderer);
            assert!(fb.iter().any(|&p| p != 0), "page {page} drew blank");
            // Text/widgets must actually paint over the BG fill
            // (0x0842): a page of pure background is the failure
            // mode of transparent-alpha labels (white/black screen
            // on device despite "successful" draws).
            let non_bg = fb.iter().filter(|&&p| p != 0x0842).count();
            assert!(
                non_bg > 100,
                "page {page}: only {non_bg} non-BG pixels (text missing?)"
            );
        }
    }

    /// Production label height (12 px) must not clip glyph rows:
    /// rendering the same text in a tall label must produce the
    /// identical text-pixel count. (rlvgl default font is 2× =
    /// 20 px glyphs; with it, the 12 px count comes out lower —
    /// the on-device "bottom of every element missing" defect.)
    #[test]
    fn styled_label_not_clipped_at_12px() {
        use rlvgl_core::widget::Color;
        let count = |h: i32| {
            let (_, node) = wrap(styled_label(
                "HELLO 123",
                0,
                0,
                120,
                h,
                Color(255, 255, 255, 255),
            ));
            let mut fb = alloc::vec![0u16; 240 * 240];
            let mut blitter = CpuBlitter;
            let mut renderer: BlitterRenderer<'_, _, 32> =
                BlitterRenderer::new(&mut blitter, framebuffer_surface(&mut fb));
            node.draw(&mut renderer);
            fb.iter().filter(|&&p| p != 0).count()
        };
        assert_eq!(count(12), count(40), "glyphs clipped at 12 px label height");
        assert!(count(12) > 50, "no text rendered at all");
    }
}
