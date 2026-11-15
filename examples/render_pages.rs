//! Render the sandbox pages off-device to PNG screenshots.
//!
//! Uses the exact device render path (`pages::build_page` →
//! `refresh` → `BlitterRenderer` over the GC9A01-shaped framebuffer),
//! so pixels match the panel. Output: one PNG per page in
//! `images/sandbox/` plus a 3×2 montage at `images/sandbox-pages.png`,
//! all with a circular alpha mask (round panel).
//!
//! Run via `just screenshots` (stable host toolchain):
//! `cargo run --example render_pages --no-default-features --features ui`
//!
//! Telemetry is canned "looks-alive" data, disclosed as such in the
//! README caption.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use esp32_s3_touch_lcd_1_28::settings::Settings;
use esp32_s3_touch_lcd_1_28::ui::{
    display_backend::framebuffer_surface,
    new_actions,
    pages,
    state::{AppState, ImuSample, PAGE_COUNT},
};
use rlvgl_platform::CpuBlitter;
use rlvgl_platform::blit::BlitterRenderer;

const W: usize = 240;
const OUT_DIR: &str = "images/sandbox";
const NAMES: [&str; 6] = ["status", "imu", "compass", "gestures", "display", "features"];

fn canned_state() -> AppState {
    let settings = Settings {
        backlight_pct: 80,
        sleep_timeout_s: 60,
        ..Settings::default()
    };
    let mut s = AppState::new(settings, 42);
    s.battery_mv = 3920;
    s.uptime_s = 754;
    s.imu = Some(ImuSample {
        ax_mg: 210,
        ay_mg: -640,
        az_mg: 760,
        gx_mdps: 1200,
        gy_mdps: -3400,
        gz_mdps: 8600,
        temp_dc: 245,
    });
    s.steps = 1234;
    s.pedometer_supported = true;
    s.last_event = "TAP";
    s.last_gesture = "SWIPE-LEFT";
    s.gesture_history = ["SWIPE-LEFT", "TAP", "SWIPE-UP", "LONG-PRESS", "-"];
    s.last_touch = Some((150, 96));
    s.heading_deg = 137;
    s.touch_ok = true;
    s.imu_ok = true;
    s.touch_id = 105;
    s.imu_rev = 0x7C;
    s.psram_bytes = 2 * 1024 * 1024;
    s
}

fn render_page(state: &AppState, page: u8) -> Vec<u16> {
    let mut st = AppState::new(state.settings, state.settings_seq);
    // Copy telemetry fields (AppState is not Clone; rebuild quickly).
    st.page = page;
    st.battery_mv = state.battery_mv;
    st.uptime_s = state.uptime_s;
    st.imu = state.imu;
    st.steps = state.steps;
    st.pedometer_supported = state.pedometer_supported;
    st.last_event = state.last_event;
    st.last_gesture = state.last_gesture;
    st.gesture_history = state.gesture_history;
    st.last_touch = state.last_touch;
    st.heading_deg = state.heading_deg;
    st.touch_ok = state.touch_ok;
    st.imu_ok = state.imu_ok;
    st.touch_id = state.touch_id;
    st.imu_rev = state.imu_rev;
    st.psram_bytes = state.psram_bytes;

    let actions = new_actions();
    let (root, handles) = pages::build_page(page, &st, &actions);
    pages::refresh(&handles, &st, 60);
    let mut fb = vec![0u16; W * W];
    let mut blitter = CpuBlitter;
    let mut renderer: BlitterRenderer<'_, _, 32> =
        BlitterRenderer::new(&mut blitter, framebuffer_surface(&mut fb));
    root.draw(&mut renderer);
    fb
}

/// RGB565 → RGBA8, alpha 0 outside the inscribed circle.
fn to_rgba_circular(fb: &[u16]) -> Vec<u8> {
    let mut out = vec![0u8; W * W * 4];
    let c = (W / 2) as i32;
    let r2 = c * c;
    for y in 0..W as i32 {
        for x in 0..W as i32 {
            let i = (y as usize * W + x as usize) * 4;
            let (dx, dy) = (x - c, y - c);
            if dx * dx + dy * dy > r2 {
                continue; // transparent
            }
            let px = fb[y as usize * W + x as usize];
            out[i] = ((px >> 11) as u8) << 3 | ((px >> 13) as u8);
            out[i + 1] = (((px >> 5) & 0x3F) as u8) << 2 | (((px >> 9) & 0x3) as u8);
            out[i + 2] = ((px & 0x1F) as u8) << 3 | ((px >> 2) as u8 & 0x7);
            out[i + 3] = 255;
        }
    }
    out
}

fn write_png(path: &Path, rgba: &[u8], w: u32, h: u32) {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path).unwrap()), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgba).unwrap();
    println!("wrote {}", path.display());
}

fn main() {
    std::fs::create_dir_all(OUT_DIR).unwrap();
    let state = canned_state();
    let mut pages = Vec::new();
    for page in 0..PAGE_COUNT {
        let rgba = to_rgba_circular(&render_page(&state, page));
        write_png(
            &Path::new(OUT_DIR).join(format!("{page}-{}.png", NAMES[page as usize])),
            &rgba,
            W as u32,
            W as u32,
        );
        pages.push(rgba);
    }
    // Montage: 3 × 2, gap 16.
    let gap = 16usize;
    let (mw, mh) = (W * 3 + gap * 2, W * 2 + gap);
    let mut mont = vec![0u8; mw * mh * 4];
    for (i, img) in pages.iter().enumerate() {
        let (ox, oy) = ((i % 3) * (W + gap), (i / 3) * (W + gap));
        for y in 0..W {
            let src = y * W * 4;
            let dst = ((oy + y) * mw + ox) * 4;
            mont[dst..dst + W * 4].copy_from_slice(&img[src..src + W * 4]);
        }
    }
    write_png(Path::new("images/sandbox-pages.png"), &mont, mw as u32, mh as u32);
}
