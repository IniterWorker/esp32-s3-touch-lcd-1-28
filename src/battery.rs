//! Battery voltage conversion for the board's BAT_ADC divider.
//!
//! The battery node reaches GPIO1 (ADC1) through a 200k/100k divider
//! (factor 3), so with a 12-bit ADC and 3.3 V reference the Waveshare
//! docs formula is `V = 3.3 / 4096 * 3 * raw`. We divide by 4095 (the
//! full-scale code) — a 0.02 % difference, far below ADC accuracy.
//!
//! Pure functions, host-testable; the ADC read itself lives in the bins.

/// Convert a raw 12-bit ADC reading to battery millivolts.
pub fn battery_mv(raw: u16) -> u32 {
    raw as u32 * 3300 * 3 / 4095
}

/// Crude linear charge estimate: 3300 mV → 0 %, 4200 mV → 100 %,
/// clamped at both ends. Good enough for a status page; it is not a
/// fuel gauge (Li-ion discharge is not linear).
pub fn battery_pct(mv: u32) -> u8 {
    ((mv.clamp(3300, 4200) - 3300) * 100 / 900) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mv_endpoints() {
        assert_eq!(battery_mv(0), 0);
        assert_eq!(battery_mv(4095), 9900);
    }

    #[test]
    fn mv_midrange_matches_docs_formula() {
        // 1715 * 3300 * 3 / 4095 = 4145.05…
        assert!((4143..=4147).contains(&battery_mv(1715)));
    }

    #[test]
    fn pct_clamps_and_is_linear() {
        assert_eq!(battery_pct(0), 0);
        assert_eq!(battery_pct(3300), 0);
        assert_eq!(battery_pct(3750), 50);
        assert_eq!(battery_pct(4200), 100);
        assert_eq!(battery_pct(5000), 100);
    }
}
