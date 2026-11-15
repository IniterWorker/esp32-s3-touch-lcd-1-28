//! Sandbox pages: one builder per page, absolute coordinates in the
//! round-safe 168×168 center square (x/y 36..204 of the 240×240
//! panel), except full-bleed backgrounds.
//!
//! Builders return the widget tree plus typed handles the app uses
//! to refresh live values and two-way-sync control widgets without
//! rebuilding the tree. Widget callbacks push `key=value` actions;
//! slider/switch state is reconciled in [`sync_controls`] via
//! last-synced shadows (so UART-originated changes also land).

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

use rlvgl_core::WidgetNode;
use rlvgl_core::style::Style;
use rlvgl_core::widget::{Color, Rect};
use rlvgl_widgets::bar::Bar;
use rlvgl_widgets::canvas::CanvasWidget;
use rlvgl_widgets::click_area::ClickArea;
use rlvgl_widgets::container::Container;
use rlvgl_widgets::label::Label;
use rlvgl_widgets::slider::Slider;
use rlvgl_widgets::switch::Switch;

use crate::battery::battery_pct;
use crate::ui::state::{AppState, PAGE_COUNT};
use crate::ui::{Actions, push_action, styled_label, styled_title, wrap};

/// Background (near-black blue).
pub const BG: Color = Color(8, 10, 18, 255);
/// Primary text.
pub const FG: Color = Color(235, 240, 245, 255);
/// Dimmed text.
pub const DIM: Color = Color(130, 140, 155, 255);
/// Accent.
pub const ACCENT: Color = Color(64, 180, 255, 255);
/// Good/on indicator.
pub const GOOD: Color = Color(80, 220, 120, 255);

/// Live widget handles for the active page. Fields stay `None`/empty
/// for widgets the page doesn't have.
#[derive(Default)]
pub struct Handles {
    /// Hello page frame counter.
    pub hello_counter: Option<Rc<RefCell<Label>>>,
    /// Status page dynamic lines.
    pub status_lines: Vec<Rc<RefCell<Label>>>,
    /// IMU page: accel bars X/Y/Z.
    pub imu_bars: Vec<Rc<RefCell<Bar>>>,
    /// IMU page value lines.
    pub imu_lines: Vec<Rc<RefCell<Label>>>,
    /// Compass dial canvas.
    pub compass_canvas: Option<Rc<RefCell<CanvasWidget>>>,
    /// Compass text lines under the dial.
    pub compass_lines: Vec<Rc<RefCell<Label>>>,
    /// Gesture page: name + xy + history lines.
    pub gesture_lines: Vec<Rc<RefCell<Label>>>,
    /// Gesture page live-dot canvas.
    pub gesture_canvas: Option<Rc<RefCell<CanvasWidget>>>,
    /// Display page backlight slider + last-synced shadow.
    pub backlight_slider: Option<Rc<RefCell<Slider>>>,
    /// Display page invert switch.
    pub invert_switch: Option<Rc<RefCell<Switch>>>,
    /// Display page sync shadows (slider value, invert).
    pub display_synced: (Cell<i32>, Cell<bool>),
    /// Features page switches in key order (imu, pedometer, stream).
    pub feature_switches: Vec<Rc<RefCell<Switch>>>,
    /// Features page switch sync shadows, same order.
    pub feature_synced: Vec<Cell<bool>>,
    /// Features page info lines.
    pub system_lines: Vec<Rc<RefCell<Label>>>,
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn bg_root(children: Vec<WidgetNode>) -> WidgetNode {
    let mut c = Container::new(rect(0, 0, 240, 240));
    c.style = Style {
        bg_color: BG,
        ..Style::default()
    };
    let (_, mut root) = wrap(c);
    root.children = children;
    root
}

/// Title (2× font) at top + page dots at bottom of the safe square.
fn header(title: &str, state: &AppState) -> Vec<WidgetNode> {
    let (_, t) = wrap(styled_title(title, 36, 34, 168, ACCENT));
    let mut dots = String::new();
    for i in 0..PAGE_COUNT {
        dots.push(if i == state.page { 'O' } else { 'o' });
        dots.push(' ');
    }
    let (_, d) = wrap(styled_label(dots, 84, 196, 84, 12, DIM));
    alloc::vec![t, d]
}

fn line_label(
    kids: &mut Vec<WidgetNode>,
    store: &mut Vec<Rc<RefCell<Label>>>,
    text: impl Into<String>,
    y: i32,
    color: Color,
) {
    let (h, n) = wrap(styled_label(text, 36, y, 168, 12, color));
    store.push(h);
    kids.push(n);
}

/// Composite button: filled rect + small-font label + click area.
/// (rlvgl's `Button` hides its label with no font knob, and its 2×
/// font overflows these bounds; composing keeps text legible.)
fn action_button(
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    actions: &Actions,
    key: &'static str,
    value: &'static str,
) -> WidgetNode {
    let mut bg = Container::new(rect(x, y, w, 20));
    bg.style = Style {
        bg_color: Color(38, 48, 66, 255),
        border_color: ACCENT,
        border_width: 1,
        ..Style::default()
    };
    let (_, mut root) = wrap(bg);
    let (_, lbl) = wrap(styled_label(text, x + 4, y + 5, w - 8, 12, FG));
    let mut click = ClickArea::new(rect(x, y, w, 20));
    let a = actions.clone();
    click.set_on_click(move |_| push_action(&a, key, value));
    let (_, click_node) = wrap(click);
    root.children = alloc::vec![lbl, click_node];
    root
}

// ---------------------------------------------------------------------
// Page builders
// ---------------------------------------------------------------------

fn swatch(x: i32, color: Color, name: &str) -> Vec<WidgetNode> {
    let mut c = Container::new(rect(x, 96, 40, 40));
    c.style = Style {
        bg_color: color,
        ..Style::default()
    };
    let (_, box_node) = wrap(c);
    let (_, lbl) = wrap(styled_label(name, x, 140, 44, 12, FG));
    alloc::vec![box_node, lbl]
}

/// Hello page (bring-up proof; also the fallback for unknown pages).
pub fn build_hello(state: &AppState, _actions: &Actions) -> (WidgetNode, Handles) {
    let mut kids = header("SBX HELLO", state);
    kids.extend(swatch(52, Color(255, 0, 0, 255), "RED"));
    kids.extend(swatch(100, Color(0, 255, 0, 255), "GREEN"));
    kids.extend(swatch(148, Color(0, 0, 255, 255), "BLUE"));
    let (counter, counter_node) = wrap(styled_label("frame 0", 36, 162, 168, 12, FG));
    kids.push(counter_node);
    let handles = Handles {
        hello_counter: Some(counter),
        ..Handles::default()
    };
    (bg_root(kids), handles)
}

fn build_status(state: &AppState, _actions: &Actions) -> (WidgetNode, Handles) {
    let mut kids = header("STATUS", state);
    let mut handles = Handles::default();
    for i in 0..5 {
        line_label(&mut kids, &mut handles.status_lines, "-", 62 + i * 18, FG);
    }
    (bg_root(kids), handles)
}

fn build_imu(state: &AppState, _actions: &Actions) -> (WidgetNode, Handles) {
    let mut kids = header("IMU LIVE", state);
    let mut handles = Handles::default();
    let (_, l) = wrap(styled_label("AX      AY      AZ", 36, 58, 168, 12, DIM));
    kids.push(l);
    for i in 0..3 {
        let mut bar = Bar::new(rect(36 + i * 58, 74, 10, 60), -1000, 1000);
        bar.set_value(0);
        let (h, n) = wrap(bar);
        handles.imu_bars.push(h);
        kids.push(n);
    }
    for i in 0..4 {
        line_label(&mut kids, &mut handles.imu_lines, "-", 142 + i * 14, FG);
    }
    (bg_root(kids), handles)
}

fn build_compass(state: &AppState, _actions: &Actions) -> (WidgetNode, Handles) {
    let mut kids = header("COMPASS", state);
    let mut handles = Handles::default();
    let mut cv = CanvasWidget::new(rect(66, 52, 108, 108), 108, 108);
    cv.fill(BG);
    let (h, n) = wrap(cv);
    handles.compass_canvas = Some(h);
    kids.push(n);
    for i in 0..3 {
        line_label(&mut kids, &mut handles.compass_lines, "-", 164 + i * 12, FG);
    }
    (bg_root(kids), handles)
}

fn build_gestures(state: &AppState, _actions: &Actions) -> (WidgetNode, Handles) {
    let mut kids = header("GESTURES", state);
    let mut handles = Handles::default();
    line_label(&mut kids, &mut handles.gesture_lines, "-", 58, ACCENT);
    line_label(&mut kids, &mut handles.gesture_lines, "-", 74, FG);
    let mut cv = CanvasWidget::new(rect(66, 92, 108, 72), 108, 72);
    cv.fill(BG);
    let (h, n) = wrap(cv);
    handles.gesture_canvas = Some(h);
    kids.push(n);
    for i in 0..3 {
        line_label(
            &mut kids,
            &mut handles.gesture_lines,
            "-",
            168 + i * 12,
            DIM,
        );
    }
    (bg_root(kids), handles)
}

fn build_display(state: &AppState, actions: &Actions) -> (WidgetNode, Handles) {
    let mut kids = header("DISPLAY", state);
    let mut handles = Handles::default();
    let s = state.settings;

    let (_, l) = wrap(styled_label("BACKLIGHT", 36, 56, 168, 12, DIM));
    kids.push(l);
    let mut slider = Slider::new(rect(36, 70, 168, 18), 0, 100);
    slider.set_value(i32::from(s.backlight_pct));
    let (h, n) = wrap(slider);
    handles.display_synced.0.set(i32::from(s.backlight_pct));
    handles.backlight_slider = Some(h);
    kids.push(n);

    let (_, l) = wrap(styled_label("ROTATION", 36, 94, 168, 12, DIM));
    kids.push(l);
    for (i, deg) in ["0", "90", "180", "270"].into_iter().enumerate() {
        kids.push(action_button(
            deg,
            36 + i as i32 * 43,
            108,
            40,
            actions,
            "rotation",
            deg,
        ));
    }

    let (_, l) = wrap(styled_label("INVERT", 36, 136, 100, 12, DIM));
    kids.push(l);
    let mut sw = Switch::new(rect(150, 134, 44, 20));
    sw.set_on(s.invert);
    let (h, n) = wrap(sw);
    handles.display_synced.1.set(s.invert);
    handles.invert_switch = Some(h);
    kids.push(n);

    let (_, l) = wrap(styled_label("SLEEP S", 36, 162, 168, 12, DIM));
    kids.push(l);
    for (i, v) in ["0", "15", "30", "60", "120"].into_iter().enumerate() {
        kids.push(action_button(
            v,
            36 + i as i32 * 34,
            176,
            32,
            actions,
            "sleep_s",
            v,
        ));
    }
    (bg_root(kids), handles)
}

/// Feature switches, in row order. Touch is deliberately absent:
/// it is always on (a disable toggle can strand the user).
const FEATURE_KEYS: [&str; 3] = ["imu", "pedometer", "stream"];

fn feature_value(s: &AppState, idx: usize) -> bool {
    match idx {
        0 => s.settings.imu_enabled,
        1 => s.settings.pedometer,
        _ => s.settings.stream_imu,
    }
}

fn build_features(state: &AppState, actions: &Actions) -> (WidgetNode, Handles) {
    let mut kids = header("FEATURES", state);
    let mut handles = Handles::default();
    for (i, key) in FEATURE_KEYS.into_iter().enumerate() {
        let y = 56 + i as i32 * 28;
        let (_, l) = wrap(styled_label(
            key.to_ascii_uppercase(),
            36,
            y + 3,
            100,
            12,
            FG,
        ));
        kids.push(l);
        let mut sw = Switch::new(rect(150, y, 44, 20));
        let v = feature_value(state, i);
        sw.set_on(v);
        let (h, n) = wrap(sw);
        handles.feature_switches.push(h);
        handles.feature_synced.push(Cell::new(v));
        kids.push(n);
    }
    let (_, l) = wrap(styled_label("IMU ODR", 36, 154, 60, 12, DIM));
    kids.push(l);
    for (i, v) in ["0", "1", "2", "3"].into_iter().enumerate() {
        kids.push(action_button(
            v,
            96 + i as i32 * 28,
            150,
            26,
            actions,
            "odr",
            v,
        ));
    }
    line_label(&mut kids, &mut handles.system_lines, "-", 172, DIM);
    line_label(&mut kids, &mut handles.system_lines, "-", 183, DIM);
    (bg_root(kids), handles)
}

/// Build the page tree for `page` (0 Status, 1 IMU, 2 Compass,
/// 3 Gestures, 4 Display, 5 Features).
pub fn build_page(page: u8, state: &AppState, actions: &Actions) -> (WidgetNode, Handles) {
    match page {
        0 => build_status(state, actions),
        1 => build_imu(state, actions),
        2 => build_compass(state, actions),
        3 => build_gestures(state, actions),
        4 => build_display(state, actions),
        5 => build_features(state, actions),
        _ => build_hello(state, actions),
    }
}

// ---------------------------------------------------------------------
// Tick-time refresh + control sync
// ---------------------------------------------------------------------

fn set_line(lines: &[Rc<RefCell<Label>>], idx: usize, text: String) {
    if let Some(l) = lines.get(idx) {
        let mut l = l.borrow_mut();
        if l.text() != text {
            l.set_text(text);
        }
    }
}

const fn onoff(b: bool) -> &'static str {
    if b { "O" } else { "-" }
}

/// Refresh live (non-control) widget values from state.
pub fn refresh(handles: &Handles, state: &AppState, frame: u32) {
    if let Some(c) = &handles.hello_counter {
        c.borrow_mut().set_text(format!("frame {frame}"));
    }
    if !handles.status_lines.is_empty() {
        let s = &state.settings;
        set_line(
            &handles.status_lines,
            0,
            format!(
                "BATT {}% {}mV",
                battery_pct(state.battery_mv),
                state.battery_mv
            ),
        );
        set_line(
            &handles.status_lines,
            1,
            format!(
                "BL {}% ROT {}",
                s.backlight_pct,
                u16::from(s.rotation % 4) * 90
            ),
        );
        set_line(
            &handles.status_lines,
            2,
            format!("UPTIME {}s", state.uptime_s),
        );
        set_line(
            &handles.status_lines,
            3,
            format!(
                "IMU{} TCH{} PED{}",
                onoff(s.imu_enabled && state.imu_ok),
                onoff(state.touch_ok),
                onoff(s.pedometer && state.pedometer_supported),
            ),
        );
        set_line(&handles.status_lines, 4, format!("STEPS {}", state.steps));
    }
    if !handles.imu_bars.is_empty() {
        match state.imu {
            Some(s) => {
                let vals = [s.ax_mg, s.ay_mg, s.az_mg];
                for (bar, v) in handles.imu_bars.iter().zip(vals) {
                    bar.borrow_mut().set_value(v.clamp(-1000, 1000));
                }
                set_line(
                    &handles.imu_lines,
                    0,
                    format!("a {} {} {} mg", s.ax_mg, s.ay_mg, s.az_mg),
                );
                set_line(
                    &handles.imu_lines,
                    1,
                    format!(
                        "g {} {} {} dps",
                        s.gx_mdps / 1000,
                        s.gy_mdps / 1000,
                        s.gz_mdps / 1000
                    ),
                );
                set_line(
                    &handles.imu_lines,
                    2,
                    format!(
                        "steps {}  {}.{}C",
                        state.steps,
                        s.temp_dc / 10,
                        (s.temp_dc % 10).abs()
                    ),
                );
            }
            None => {
                set_line(&handles.imu_lines, 0, String::from("IMU OFF/ABSENT"));
                set_line(&handles.imu_lines, 1, String::new());
                set_line(&handles.imu_lines, 2, String::new());
            }
        }
        set_line(&handles.imu_lines, 3, format!("EVENT {}", state.last_event));
    }
    if let Some(cv) = &handles.compass_canvas {
        draw_compass(&mut cv.borrow_mut(), state);
        let card = cardinal(state.heading_deg);
        set_line(
            &handles.compass_lines,
            0,
            format!("HDG {:03} {}", norm_deg(state.heading_deg), card),
        );
        if let Some(s) = state.imu {
            set_line(
                &handles.compass_lines,
                1,
                format!(
                    "PITCH {} ROLL {}",
                    atan2_deg(s.ay_mg, s.az_mg),
                    atan2_deg(s.ax_mg, s.az_mg)
                ),
            );
        }
        set_line(
            &handles.compass_lines,
            2,
            String::from("REL - NO MAG - TAP=ZERO"),
        );
    }
    if !handles.gesture_lines.is_empty() {
        set_line(&handles.gesture_lines, 0, String::from(state.last_gesture));
        let xy = match state.last_touch {
            Some((x, y)) => format!("x={x} y={y}"),
            None => String::from("x=- y=-"),
        };
        set_line(&handles.gesture_lines, 1, xy);
        for i in 0..3 {
            set_line(
                &handles.gesture_lines,
                2 + i,
                String::from(state.gesture_history.get(i + 1).copied().unwrap_or("-")),
            );
        }
        if let Some(cv) = &handles.gesture_canvas {
            let mut cv = cv.borrow_mut();
            cv.fill(BG);
            if let Some((x, y)) = state.last_touch {
                let px = x * 108 / 240;
                let py = y * 72 / 240;
                for dy in -2..=2 {
                    for dx in -2..=2 {
                        cv.draw_pixel(px + dx, py + dy, GOOD);
                    }
                }
            }
        }
    }
    if !handles.system_lines.is_empty() {
        set_line(
            &handles.system_lines,
            0,
            format!(
                "p{} r{:02x} seq {}",
                state.touch_id, state.imu_rev, state.settings_seq
            ),
        );
        set_line(
            &handles.system_lines,
            1,
            format!("flash16M psram {}K", state.psram_bytes / 1024),
        );
    }
}

/// Two-way sync of slider/switches with state (shadow-tracked).
pub fn sync_controls(handles: &Handles, state: &mut AppState) {
    if let Some(slider) = &handles.backlight_slider {
        let synced = handles.display_synced.0.get();
        let widget = slider.borrow().value();
        let want = i32::from(state.settings.backlight_pct);
        if widget != synced {
            let _ = state.apply("backlight", &format!("{widget}"));
            handles.display_synced.0.set(widget);
        } else if want != synced {
            slider.borrow_mut().set_value(want);
            handles.display_synced.0.set(want);
        }
    }
    if let Some(sw) = &handles.invert_switch {
        sync_switch(
            sw,
            &handles.display_synced.1,
            state.settings.invert,
            "invert",
            state,
        );
    }
    for (i, (sw, cell)) in handles
        .feature_switches
        .iter()
        .zip(handles.feature_synced.iter())
        .enumerate()
    {
        let want = feature_value(state, i);
        let key = FEATURE_KEYS[i];
        sync_switch(sw, cell, want, key, state);
    }
}

fn sync_switch(
    sw: &Rc<RefCell<Switch>>,
    synced: &Cell<bool>,
    want: bool,
    key: &'static str,
    state: &mut AppState,
) {
    let widget = sw.borrow().is_on();
    if widget != synced.get() {
        let _ = state.apply(key, if widget { "on" } else { "off" });
        synced.set(widget);
    } else if want != widget {
        sw.borrow_mut().set_on(want);
        synced.set(want);
    }
}

// ---------------------------------------------------------------------
// Compass helpers (fixed-point unit circle; no libm in core-only)
// ---------------------------------------------------------------------

/// 16-point unit circle, (sin, cos) × 256 at 22.5° steps starting North.
const CIRCLE16: [(i32, i32); 16] = [
    (0, 256),
    (98, 237),
    (181, 181),
    (237, 98),
    (256, 0),
    (237, -98),
    (181, -181),
    (98, -237),
    (0, -256),
    (-98, -237),
    (-181, -181),
    (-237, -98),
    (-256, 0),
    (-237, 98),
    (-181, 181),
    (-98, 237),
];

fn norm_deg(d: i32) -> i32 {
    d.rem_euclid(360)
}

fn cardinal(deg: i32) -> &'static str {
    const NAMES: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    NAMES[(((norm_deg(deg) + 22) % 360) / 45) as usize]
}

/// atan2 in whole degrees via a rational approximation (|err| ~1°).
fn atan2_deg(y: i32, x: i32) -> i32 {
    if x == 0 {
        return if y > 0 {
            90
        } else if y < 0 {
            -90
        } else {
            0
        };
    }
    let z = y as f32 / x as f32;
    let az = z.abs();
    // atan(z) ≈ z·(π/4 + 0.273·(1−|z|)) for |z| ≤ 1; reciprocal form else.
    let a = if az <= 1.0 {
        z * (core::f32::consts::FRAC_PI_4 + 0.273 * (1.0 - az))
    } else {
        core::f32::consts::FRAC_PI_2
            - (1.0 / z) * (core::f32::consts::FRAC_PI_4 + 0.273 * (1.0 - 1.0 / az))
    };
    let deg = a * 57.295_78;
    let deg = if x < 0 {
        if y >= 0 { deg + 180.0 } else { deg - 180.0 }
    } else {
        deg
    };
    deg as i32
}

fn draw_compass(cv: &mut CanvasWidget, state: &AppState) {
    cv.fill(BG);
    let c = 54;
    let r = 48;
    // Dial ring (segments between circle points) + cardinal ticks.
    for (i, (s, co)) in CIRCLE16.iter().enumerate() {
        let x = c + s * r / 256;
        let y = c - co * r / 256;
        let (s2, co2) = CIRCLE16[(i + 1) % 16];
        cv.draw_line(x, y, c + s2 * r / 256, c - co2 * r / 256, DIM);
        if i % 4 == 0 {
            let x2 = c + s * (r - 6) / 256;
            let y2 = c - co * (r - 6) / 256;
            cv.draw_line(x, y, x2, y2, FG);
        }
    }
    // Needle toward relative heading.
    let idx = ((norm_deg(state.heading_deg) + 11) % 360 / 22) as usize % 16;
    let (s, co) = CIRCLE16[idx];
    cv.draw_line(
        c,
        c,
        c + s * (r - 10) / 256,
        c - co * (r - 10) / 256,
        ACCENT,
    );
    cv.draw_pixel(c, c, FG);
}
