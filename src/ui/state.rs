//! Sandbox application state + the single mutation entry point.
//!
//! Both the touch UI (widget actions) and the UART protocol funnel
//! through [`AppState::apply`], so validation rules exist exactly once.
//! No rlvgl or ESP deps here — host tests cover every key.

use crate::settings::Settings;

/// Live IMU sample as the UI consumes it (integer milli-units; the
/// sensor task converts from the driver's floats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuSample {
    /// Accelerometer X, milli-g.
    pub ax_mg: i32,
    /// Accelerometer Y, milli-g.
    pub ay_mg: i32,
    /// Accelerometer Z, milli-g.
    pub az_mg: i32,
    /// Gyroscope X, milli-dps.
    pub gx_mdps: i32,
    /// Gyroscope Y, milli-dps.
    pub gy_mdps: i32,
    /// Gyroscope Z, milli-dps.
    pub gz_mdps: i32,
    /// Temperature, deci-°C.
    pub temp_dc: i32,
}

/// Why an [`AppState::apply`] call was rejected. State is never
/// partially mutated on error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// Key not recognized.
    UnknownKey,
    /// Value unparsable or out of range for the key.
    BadValue,
}

/// Number of UI pages.
pub const PAGE_COUNT: u8 = 6;

/// Whole sandbox state: persisted [`Settings`] + volatile telemetry.
pub struct AppState {
    /// Persisted settings (saved to flash on change).
    pub settings: Settings,
    /// Settings sequence number as loaded/last saved (0 = defaults).
    pub settings_seq: u32,
    /// Active page index `0..PAGE_COUNT`.
    pub page: u8,
    /// Latest battery voltage, mV.
    pub battery_mv: u32,
    /// Latest IMU sample, if the sensor is live.
    pub imu: Option<ImuSample>,
    /// Pedometer step count (0 when unsupported/disabled).
    pub steps: u32,
    /// Whether the fitted IMU implements the pedometer engine.
    pub pedometer_supported: bool,
    /// Last IMU activity event name ("TAP"/"MOTION"/"-").
    pub last_event: &'static str,
    /// Last derived gesture name (trajectory-based; see ui::input).
    pub last_gesture: &'static str,
    /// Recent gesture names, newest first (gesture page history).
    pub gesture_history: [&'static str; 5],
    /// Last touch coordinates reported.
    pub last_touch: Option<(i32, i32)>,
    /// Touch controller probed OK (effective availability).
    pub touch_ok: bool,
    /// IMU probed OK (effective availability).
    pub imu_ok: bool,
    /// Integrated relative heading, degrees 0..360 (compass page).
    pub heading_deg: i32,
    /// Touch controller chip id byte (system page), 0 = unknown.
    pub touch_id: u8,
    /// IMU revision byte (system page), 0 = unknown.
    pub imu_rev: u8,
    /// PSRAM bytes detected at boot (0 = none / SRAM-only).
    pub psram_bytes: usize,
    /// Firmware uptime, seconds.
    pub uptime_s: u32,
    /// Monotonic revision: bumped on every mutation that should
    /// schedule a redraw/flush and (for settings keys) a flash save.
    pub rev: u32,
    /// Revision at which settings were last handed to the saver task.
    pub settings_dirty_from_rev: u32,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(Settings::default(), 0)
    }
}

impl AppState {
    /// New state from loaded settings.
    pub fn new(settings: Settings, settings_seq: u32) -> Self {
        Self {
            settings,
            settings_seq,
            page: 0,
            battery_mv: 0,
            imu: None,
            steps: 0,
            pedometer_supported: false,
            last_event: "-",
            last_gesture: "-",
            gesture_history: ["-"; 5],
            last_touch: None,
            touch_ok: false,
            imu_ok: false,
            heading_deg: 0,
            touch_id: 0,
            imu_rev: 0,
            psram_bytes: 0,
            uptime_s: 0,
            rev: 0,
            settings_dirty_from_rev: 0,
        }
    }

    /// Apply one `key=value` mutation. Keys/values:
    ///
    /// - `backlight`: `0..=100`
    /// - `rotation`: `0|90|180|270` (degrees)
    /// - `invert`, `imu`, `pedometer`, `stream`: `on|off` (touch has
    ///   no key by design: it is always on)
    /// - `sleep_s`: `0..=3600`
    /// - `odr`: `0..=3` (IMU ODR index)
    /// - `page`: `0..=5`
    ///
    /// Settings keys mark the settings dirty (debounced flash save);
    /// `page` is volatile. Every accepted call bumps [`Self::rev`].
    pub fn apply(&mut self, key: &str, value: &str) -> Result<(), ApplyError> {
        // Parse + validate fully before mutating anything.
        enum M {
            Settings(Settings),
            Page(u8),
        }
        let m = match key {
            "backlight" => {
                let v: u32 = parse(value)?;
                if v > 100 {
                    return Err(ApplyError::BadValue);
                }
                let mut s = self.settings;
                s.backlight_pct = v as u8;
                M::Settings(s)
            }
            "rotation" => {
                let idx = match value {
                    "0" => 0,
                    "90" => 1,
                    "180" => 2,
                    "270" => 3,
                    _ => return Err(ApplyError::BadValue),
                };
                let mut s = self.settings;
                s.rotation = idx;
                M::Settings(s)
            }
            "invert" | "imu" | "pedometer" | "stream" => {
                let on = parse_onoff(value)?;
                let mut s = self.settings;
                match key {
                    "invert" => s.invert = on,
                    "imu" => s.imu_enabled = on,
                    "pedometer" => s.pedometer = on,
                    _ => s.stream_imu = on,
                }
                M::Settings(s)
            }
            "sleep_s" => {
                let v: u32 = parse(value)?;
                if v > 3600 {
                    return Err(ApplyError::BadValue);
                }
                let mut s = self.settings;
                s.sleep_timeout_s = v as u16;
                M::Settings(s)
            }
            "odr" => {
                let v: u32 = parse(value)?;
                if v > 3 {
                    return Err(ApplyError::BadValue);
                }
                let mut s = self.settings;
                s.imu_odr_idx = v as u8;
                M::Settings(s)
            }
            "page" => {
                let v: u32 = parse(value)?;
                if v >= u32::from(PAGE_COUNT) {
                    return Err(ApplyError::BadValue);
                }
                M::Page(v as u8)
            }
            _ => return Err(ApplyError::UnknownKey),
        };
        match m {
            M::Settings(s) => {
                self.settings = s;
                self.settings_dirty_from_rev = self.rev;
            }
            M::Page(p) => self.page = p,
        }
        self.rev = self.rev.wrapping_add(1);
        Ok(())
    }

    /// Current value of a key as printed by `SBX GET` / echoed in
    /// `SBX OK`. Buffer-free: writes into `buf`, returns the slice.
    pub fn get_str<'a>(&self, key: &str, buf: &'a mut [u8; 16]) -> Option<&'a str> {
        let s = &self.settings;
        let v: &str = match key {
            "backlight" => fmt_u32(buf, u32::from(s.backlight_pct)),
            "rotation" => fmt_u32(buf, u32::from(s.rotation % 4) * 90),
            "invert" => onoff(s.invert),
            "imu" => onoff(s.imu_enabled),
            "pedometer" => onoff(s.pedometer),
            "stream" => onoff(s.stream_imu),
            "sleep_s" => fmt_u32(buf, u32::from(s.sleep_timeout_s)),
            "odr" => fmt_u32(buf, u32::from(s.imu_odr_idx)),
            "page" => fmt_u32(buf, u32::from(self.page)),
            "battery_mv" => fmt_u32(buf, self.battery_mv),
            _ => return None,
        };
        Some(v)
    }

    /// All `SBX GET` keys, in print order.
    pub const GET_KEYS: [&'static str; 10] = [
        "backlight",
        "rotation",
        "invert",
        "imu",
        "pedometer",
        "stream",
        "sleep_s",
        "odr",
        "page",
        "battery_mv",
    ];
}

fn parse(value: &str) -> Result<u32, ApplyError> {
    value.parse::<u32>().map_err(|_| ApplyError::BadValue)
}

fn parse_onoff(value: &str) -> Result<bool, ApplyError> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(ApplyError::BadValue),
    }
}

const fn onoff(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn fmt_u32(buf: &mut [u8; 16], v: u32) -> &str {
    let mut tmp = [0u8; 10];
    let mut n = 0;
    let mut v = v;
    loop {
        tmp[9 - n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    buf[..n].copy_from_slice(&tmp[10 - n..]);
    // SAFETY: digits are ASCII.
    unsafe { core::str::from_utf8_unchecked(&buf[..n]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> AppState {
        AppState::default()
    }

    #[test]
    fn backlight_bounds() {
        let mut s = st();
        assert!(s.apply("backlight", "0").is_ok());
        assert_eq!(s.settings.backlight_pct, 0);
        assert!(s.apply("backlight", "100").is_ok());
        assert_eq!(s.apply("backlight", "101"), Err(ApplyError::BadValue));
        assert_eq!(s.apply("backlight", "-1"), Err(ApplyError::BadValue));
        assert_eq!(s.apply("backlight", "x"), Err(ApplyError::BadValue));
        assert_eq!(s.settings.backlight_pct, 100); // unchanged on error
    }

    #[test]
    fn rotation_degrees_to_index() {
        let mut s = st();
        for (deg, idx) in [("0", 0), ("90", 1), ("180", 2), ("270", 3)] {
            assert!(s.apply("rotation", deg).is_ok());
            assert_eq!(s.settings.rotation, idx);
        }
        assert_eq!(s.apply("rotation", "45"), Err(ApplyError::BadValue));
        assert_eq!(s.apply("rotation", "2"), Err(ApplyError::BadValue));
    }

    #[test]
    fn onoff_keys() {
        let mut s = st();
        assert!(s.apply("invert", "on").is_ok() && s.settings.invert);
        assert!(s.apply("imu", "off").is_ok() && !s.settings.imu_enabled);
        assert!(s.apply("pedometer", "off").is_ok() && !s.settings.pedometer);
        // No touch toggle by design: the key must be rejected.
        assert_eq!(s.apply("touch", "off"), Err(ApplyError::UnknownKey));
        assert!(s.apply("stream", "on").is_ok() && s.settings.stream_imu);
        assert_eq!(s.apply("imu", "1"), Err(ApplyError::BadValue));
        assert_eq!(s.apply("imu", "true"), Err(ApplyError::BadValue));
    }

    #[test]
    fn sleep_and_odr_and_page_bounds() {
        let mut s = st();
        assert!(s.apply("sleep_s", "3600").is_ok());
        assert_eq!(s.apply("sleep_s", "3601"), Err(ApplyError::BadValue));
        assert!(s.apply("odr", "3").is_ok());
        assert_eq!(s.apply("odr", "4"), Err(ApplyError::BadValue));
        assert!(s.apply("page", "5").is_ok());
        assert_eq!(s.apply("page", "6"), Err(ApplyError::BadValue));
    }

    #[test]
    fn unknown_key_and_rev_bumps() {
        let mut s = st();
        let rev = s.rev;
        assert_eq!(s.apply("nope", "1"), Err(ApplyError::UnknownKey));
        assert_eq!(s.rev, rev);
        assert!(s.apply("page", "1").is_ok());
        assert_eq!(s.rev, rev + 1);
    }

    #[test]
    fn get_str_roundtrip() {
        let mut s = st();
        let mut buf = [0u8; 16];
        assert_eq!(s.get_str("backlight", &mut buf), Some("100"));
        assert_eq!(s.get_str("rotation", &mut buf), Some("180"));
        assert_eq!(s.get_str("stream", &mut buf), Some("off"));
        s.apply("rotation", "270").unwrap();
        assert_eq!(s.get_str("rotation", &mut buf), Some("270"));
        assert_eq!(s.get_str("bogus", &mut buf), None);
    }
}
