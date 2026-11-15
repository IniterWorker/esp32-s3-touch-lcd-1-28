//! Touch input conversion: CST816S reports → rlvgl `Event`s, plus
//! trajectory-derived gestures.
//!
//! Hardware fact (verified 2026-11-15, CST816T proj105 fw2): the
//! module firmware never sets the gesture byte — every report is a
//! plain point. Gestures (tap, swipe, long-press) are therefore
//! derived here from coordinate trajectories, which is also why page
//! swipe uses this recognizer and not the driver's gesture enum.

use alloc::vec::Vec;
use rlvgl_core::event::Event;

/// One touch-controller report, driver-agnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchReport {
    /// X coordinate, panel pixels.
    pub x: i32,
    /// Y coordinate, panel pixels.
    pub y: i32,
    /// Finger currently down.
    pub pressed: bool,
}

/// Last valid panel coordinate (240×240 panel).
const MAX: i32 = 239;

/// Map a raw touch-controller point into display-logical coordinates
/// for display rotation `rotation_idx` (0..3 → 0/90/180/270°), so
/// touch follows the screen when the user rotates it.
///
/// Derivation: raw CST816 coordinates coincide with logical coords
/// at Rotate180 (verified on device at the default). gc9a01-rs
/// implements rotation as MADCTL MX/MY flips plus a transposed
/// framebuffer for 90/270 (MV always 0); composing those mappings
/// (logical → GRAM address) and inverting against the Rotate180
/// anchor gives the table below. Inputs are clamped to the panel.
pub fn rotate_touch(x: i32, y: i32, rotation_idx: u8) -> (i32, i32) {
    let x = x.clamp(0, MAX);
    let y = y.clamp(0, MAX);
    match rotation_idx % 4 {
        0 => (MAX - x, MAX - y),
        1 => (y, MAX - x),
        2 => (x, y),
        _ => (MAX - y, x),
    }
}

impl GestureTracker {
    /// Forget any in-progress contact (e.g. after a display rotation
    /// change, when tracked coordinates change frame mid-gesture).
    pub fn reset(&mut self) {
        self.down_at = None;
    }
}

/// Gesture derived from a touch trajectory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Short press released near its origin.
    Tap {
        /// Release X.
        x: i32,
        /// Release Y.
        y: i32,
    },
    /// Release ≥ threshold left of origin.
    SwipeLeft,
    /// Release ≥ threshold right of origin.
    SwipeRight,
    /// Release ≥ threshold above origin.
    SwipeUp,
    /// Release ≥ threshold below origin.
    SwipeDown,
    /// Held in place beyond the long-press time (fired once, on release
    /// or while held — here: on release, if no swipe).
    LongPress {
        /// Release X.
        x: i32,
        /// Release Y.
        y: i32,
    },
}

impl Gesture {
    /// Short display name.
    pub const fn name(self) -> &'static str {
        match self {
            Gesture::Tap { .. } => "TAP",
            Gesture::SwipeLeft => "SWIPE-LEFT",
            Gesture::SwipeRight => "SWIPE-RIGHT",
            Gesture::SwipeUp => "SWIPE-UP",
            Gesture::SwipeDown => "SWIPE-DOWN",
            Gesture::LongPress { .. } => "LONG-PRESS",
        }
    }
}

/// Minimum travel (px) for a release to count as a swipe.
const SWIPE_MIN_PX: i32 = 40;
/// Maximum travel (px) for a release to still count as a tap.
const TAP_MAX_PX: i32 = 12;
/// Minimum hold (ticks, caller-defined units) for a long press.
const LONG_PRESS_TICKS: u32 = 40;

/// Stateful trajectory recognizer. Feed every report; get back the
/// rlvgl events to dispatch (in order) and any completed gesture.
#[derive(Default)]
pub struct GestureTracker {
    down_at: Option<(i32, i32, u32)>,
}

impl GestureTracker {
    /// Feed one report at tick `now` (monotonic, any unit — only
    /// differences vs [`LONG_PRESS_TICKS`] matter).
    pub fn feed(&mut self, r: TouchReport, now: u32) -> (Vec<Event>, Option<Gesture>) {
        let mut events = Vec::new();
        let mut gesture = None;
        match (r.pressed, self.down_at) {
            (true, None) => {
                self.down_at = Some((r.x, r.y, now));
                events.push(Event::PointerDown { x: r.x, y: r.y });
                events.push(Event::PressDown { x: r.x, y: r.y });
            }
            (true, Some(_)) => {
                events.push(Event::PointerMove { x: r.x, y: r.y });
            }
            (false, Some((ox, oy, t0))) => {
                self.down_at = None;
                events.push(Event::PointerUp { x: r.x, y: r.y });
                let dx = r.x - ox;
                let dy = r.y - oy;
                let (adx, ady) = (dx.abs(), dy.abs());
                if adx.max(ady) >= SWIPE_MIN_PX {
                    gesture = Some(if adx >= ady {
                        if dx < 0 {
                            Gesture::SwipeLeft
                        } else {
                            Gesture::SwipeRight
                        }
                    } else if dy < 0 {
                        Gesture::SwipeUp
                    } else {
                        Gesture::SwipeDown
                    });
                    // No PressRelease: a swipe must not click whatever
                    // widget sits under the release point.
                } else if adx.max(ady) <= TAP_MAX_PX {
                    events.push(Event::PressRelease { x: r.x, y: r.y });
                    gesture = Some(if now.wrapping_sub(t0) >= LONG_PRESS_TICKS {
                        Gesture::LongPress { x: r.x, y: r.y }
                    } else {
                        Gesture::Tap { x: r.x, y: r.y }
                    });
                }
            }
            (false, None) => {}
        }
        (events, gesture)
    }

    /// Whether a finger is currently tracked as down.
    pub fn is_down(&self) -> bool {
        self.down_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(x: i32, y: i32, pressed: bool) -> TouchReport {
        TouchReport { x, y, pressed }
    }

    #[test]
    fn tap_emits_press_release() {
        let mut g = GestureTracker::default();
        let (ev, _) = g.feed(rep(10, 10, true), 0);
        assert!(matches!(ev[0], Event::PointerDown { .. }));
        let (ev, gest) = g.feed(rep(11, 10, false), 3);
        assert!(ev.iter().any(|e| matches!(e, Event::PressRelease { .. })));
        assert_eq!(gest, Some(Gesture::Tap { x: 11, y: 10 }));
    }

    #[test]
    fn swipe_suppresses_click() {
        let mut g = GestureTracker::default();
        g.feed(rep(150, 120, true), 0);
        g.feed(rep(100, 120, true), 1);
        let (ev, gest) = g.feed(rep(60, 118, false), 2);
        assert_eq!(gest, Some(Gesture::SwipeLeft));
        assert!(!ev.iter().any(|e| matches!(e, Event::PressRelease { .. })));
    }

    #[test]
    fn swipe_directions() {
        for (to, want) in [
            ((200, 120), Gesture::SwipeRight),
            ((120, 30), Gesture::SwipeUp),
            ((120, 210), Gesture::SwipeDown),
        ] {
            let mut g = GestureTracker::default();
            g.feed(rep(120, 120, true), 0);
            let (_, gest) = g.feed(rep(to.0, to.1, false), 4);
            assert_eq!(gest, Some(want));
        }
    }

    #[test]
    fn long_press_needs_hold_time() {
        let mut g = GestureTracker::default();
        g.feed(rep(50, 50, true), 0);
        let (_, gest) = g.feed(rep(50, 50, false), LONG_PRESS_TICKS + 1);
        assert_eq!(gest, Some(Gesture::LongPress { x: 50, y: 50 }));
    }

    #[test]
    fn idle_release_is_silent() {
        let mut g = GestureTracker::default();
        let (ev, gest) = g.feed(rep(0, 0, false), 0);
        assert!(ev.is_empty() && gest.is_none());
    }
    #[test]
    fn rotate_touch_anchor_and_corners() {
        // Rotate180 is the verified identity anchor.
        assert_eq!(rotate_touch(10, 20, 2), (10, 20));
        // Rotate0 = 180° from the anchor frame.
        assert_eq!(rotate_touch(0, 0, 0), (239, 239));
        assert_eq!(rotate_touch(10, 20, 0), (229, 219));
        // 90/270 swap axes with one flip each.
        assert_eq!(rotate_touch(0, 0, 1), (0, 239));
        assert_eq!(rotate_touch(239, 0, 1), (0, 0));
        assert_eq!(rotate_touch(0, 0, 3), (239, 0));
        assert_eq!(rotate_touch(0, 239, 3), (0, 0));
    }

    #[test]
    fn rotate_touch_roundtrips() {
        // Inverting through the opposite rotation restores the point.
        for (x, y) in [(0, 0), (37, 201), (239, 239), (120, 119)] {
            let (a, b) = rotate_touch(x, y, 1);
            assert_eq!(rotate_touch(a, b, 3), (x, y));
            let (a, b) = rotate_touch(x, y, 0);
            assert_eq!(rotate_touch(a, b, 0), (x, y));
        }
    }

    #[test]
    fn rotate_touch_clamps() {
        assert_eq!(rotate_touch(-5, 999, 2), (0, 239));
    }
}
