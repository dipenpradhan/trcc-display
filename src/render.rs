//! Rendering: turn slot values into a physical-order color frame.
//!
//! The engine hands us, per active slot, a [`SlotValue`] (number + unit +
//! color). We light the profile's always-on LEDs, then for each slot encode
//! the number into its 7-segment digits, set the unit marker, and color the lit
//! LEDs. Finally the profile's wire-remap is applied.
//!
//! # Rendering pipeline
//!
//! ```text
//! SlotValue ──▶ draw_slot ──▶ 7-segment encode ──▶ logical frame
//!   (value, unit, color)         (leading-zero suppression)
//!                                                   │
//! profile.to_physical ←─────────────────────────────┘
//!   (wire remap, always-on, indicators)
//! ```
//!
//! # Leading-zero suppression
//!
//! Numbers are space-padded, not zero-padded. For a 3-digit field showing `7`,
//! the display shows "  7" (two blank digits, then `7`). This prevents
//! confusing "007" when the value is `7`.
//!
//! # Partial digits
//!
//! Two-digit slots with `partial` LEDs can show values 0..=199. Values ≥ 100
//! light the partial LEDs for the leading `1`, then show two digits.

use std::collections::HashMap;

use crate::profile::{Profile, Slot};
use crate::protocol::{Rgb, seven_seg};

/// What to draw in one slot this frame.
///
/// # Fields
///
/// * `value` — the number to display (already unit-converted; negatives clamp to 0).
/// * `unit` — unit-marker key to light (`"celsius"`, `"percent"`, …), if any.
/// * `color` — color for this slot's lit LEDs.
/// * `indicators` — extra indicator groups (profile `indicators` keys) to light with `color`.
#[derive(Debug, Clone)]
pub struct SlotValue {
    /// The number to display (already unit-converted; negatives clamp to 0).
    pub value: i64,
    /// Unit-marker key to light (`"celsius"`, `"percent"`, …), if any.
    pub unit: Option<String>,
    /// Color for this slot's lit LEDs.
    pub color: Rgb,
    /// Extra indicator groups (profile `indicators` keys) to light with `color`.
    pub indicators: Vec<String>,
}

/// Build a frame in **logical** LED order (before device remap). Use this
/// for the web preview where LED indices come from the profile definition.
///
/// # Arguments
///
/// * `profile` — the active device profile.
/// * `slots` — per-slot values to render.
/// * `indicator_color` — color for always-on/unit/indicator LEDs.
///
/// # Returns
///
/// A vector of RGB colors in logical LED order (matches profile `slots` indices).
pub fn frame_logical(
    profile: &Profile,
    slots: &HashMap<String, SlotValue>,
    indicator_color: Rgb,
) -> Vec<Rgb> {
    render_slots(profile, slots, indicator_color)
}

/// Build a full physical-order frame for `profile` from the per-slot values.
///
/// `indicator_color` lights the always-on / unit / indicator LEDs so they read
/// as a subtle frame rather than sharing each tile's alarm color.
///
/// # Arguments
///
/// * `profile` — the active device profile.
/// * `slots` — per-slot values to render.
/// * `indicator_color` — color for always-on/unit/indicator LEDs.
///
/// # Returns
///
/// A vector of RGB colors in physical LED order (ready for `protocol::data_packet`).
pub fn frame(
    profile: &Profile,
    slots: &HashMap<String, SlotValue>,
    indicator_color: Rgb,
) -> Vec<Rgb> {
    profile.to_physical(&render_slots(profile, slots, indicator_color))
}

fn render_slots(
    profile: &Profile,
    slots: &HashMap<String, SlotValue>,
    indicator_color: Rgb,
) -> Vec<Rgb> {
    let mut logical = vec![Rgb::default(); profile.mask_size];

    for &i in &profile.always_on {
        logical[i] = indicator_color;
    }

    for (name, sv) in slots {
        let Some(slot) = profile.slots.get(name) else {
            tracing::warn!(slot = name, "no such slot in profile — skipping");
            continue;
        };
        draw_slot(slot, sv, indicator_color, &mut logical);
        // Requested indicator groups (e.g. a "cpu"/"gpu" source badge).
        for ind in &sv.indicators {
            if let Some(leds) = profile.indicators.get(ind) {
                for &i in leds {
                    logical[i] = indicator_color;
                }
            }
        }
    }

    logical
}

fn draw_slot(slot: &Slot, sv: &SlotValue, indicator_color: Rgb, out: &mut [Rgb]) {
    for &i in &slot.on {
        out[i] = indicator_color;
    }
    if let Some(unit) = &sv.unit {
        if let Some(&led) = slot.unit.get(unit) {
            out[led] = indicator_color;
        }
    }

    let value = sv.value.max(0);
    let digit_count = slot.digits.len();
    let has_partial = slot.partial.is_some();

    // Text to lay across the digit fields, most-significant first.
    let text: String = if has_partial {
        // 2 digits + optional leading '1' for hundreds (0..=199 range).
        let v = value.min(199);
        if v >= 100 {
            if let Some(p) = &slot.partial {
                for &i in p {
                    out[i] = sv.color;
                }
            }
            format!("{:02}", v - 100) // no leading-zero suppression, e.g. 105 -> "05"
        } else {
            // suppress the leading zero: 5 -> " 5", 57 -> "57"
            format!("{:>2}", v)
        }
    } else {
        // N digits, space-padded (leading-zero suppression), clamped.
        let max = 10i64.pow(digit_count as u32) - 1;
        format!("{:>width$}", value.min(max), width = digit_count)
    };

    for (di, ch) in text.chars().enumerate() {
        if di >= digit_count {
            break;
        }
        let leds = &slot.digits[di];
        for (wi, on) in seven_seg(ch).iter().enumerate() {
            if *on {
                out[leds[wi]] = sv.color;
            }
        }
    }
}

/// Convenience: pick a color from base + warn/crit thresholds.
///
/// # Arguments
///
/// * `value` — the raw metric value.
/// * `base` — the default color.
/// * `warn` — amber threshold (at or above this value, color becomes amber).
/// * `crit` — red threshold (at or above this value, color becomes red).
///
/// # Returns
///
/// The appropriate color based on thresholds.
pub fn threshold_color(value: f64, base: Rgb, warn: Option<f64>, crit: Option<f64>) -> Rgb {
    if crit.is_some_and(|c| value >= c) {
        Rgb(255, 40, 40)
    } else if warn.is_some_and(|w| value >= w) {
        Rgb(255, 170, 0)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;

    /// Minimal AX120-like profile: 30 LEDs, one 3-digit slot, no remap.
    fn ax120() -> Profile {
        let json = serde_json::json!({
            "name": "TEST_AX120",
            "mask_size": 30,
            "always_on": [0, 1],
            "slots": {
                "main": {
                    "digits": [
                        [9,10,11,12,13,14,15],
                        [16,17,18,19,20,21,22],
                        [23,24,25,26,27,28,29]
                    ],
                    "unit": { "celsius": 6, "percent": 8 }
                }
            }
        });
        serde_json::from_value(json).unwrap()
    }

    fn slot(value: i64, unit: Option<&str>, color: Rgb) -> HashMap<String, SlotValue> {
        let mut m = HashMap::new();
        m.insert(
            "main".to_string(),
            SlotValue {
                value,
                unit: unit.map(str::to_string),
                color,
                indicators: vec![],
            },
        );
        m
    }

    // ── frame ───────────────────────────────────────────────────────────────

    #[test]
    fn frame_length_matches_mask() {
        let f = frame(
            &ax120(),
            &slot(57, Some("celsius"), Rgb(0, 200, 0)),
            Rgb(10, 10, 10),
        );
        assert_eq!(f.len(), 30);
    }

    #[test]
    fn frame_empty_slots() {
        let f = frame(&ax120(), &HashMap::new(), Rgb(10, 10, 10));
        assert_eq!(f.len(), 30);
        // Only always-on LEDs should be lit.
        assert_eq!(f[0], Rgb(10, 10, 10));
        assert_eq!(f[1], Rgb(10, 10, 10));
        // Everything else should be dark.
        for (i, &c) in f.iter().enumerate() {
            if i != 0 && i != 1 {
                assert_eq!(c, Rgb(0, 0, 0), "LED {i} should be dark");
            }
        }
    }

    #[test]
    fn always_on_and_unit_lit() {
        let f = frame(
            &ax120(),
            &slot(57, Some("celsius"), Rgb(0, 200, 0)),
            Rgb(10, 10, 10),
        );
        assert_eq!(f[0], Rgb(10, 10, 10)); // always-on
        assert_eq!(f[1], Rgb(10, 10, 10));
        assert_eq!(f[6], Rgb(10, 10, 10)); // celsius marker
        assert_eq!(f[8], Rgb(0, 0, 0)); // percent marker off
    }

    #[test]
    fn value_888_lights_every_digit_segment() {
        let f = frame(&ax120(), &slot(888, None, Rgb(5, 5, 5)), Rgb(10, 10, 10));
        // '8' = all 7 segments, so LEDs 9..30 are all lit.
        assert!(
            f[9..30].iter().all(|&c| c == Rgb(5, 5, 5)),
            "all digit LEDs should be lit for 888"
        );
    }

    #[test]
    fn leading_zero_suppression() {
        // value 7 in a 3-digit field → "  7": first two digits blank.
        let f = frame(&ax120(), &slot(7, None, Rgb(5, 5, 5)), Rgb(10, 10, 10));
        assert!(
            f[9..=22].iter().all(|&c| c == Rgb(0, 0, 0)),
            "leading (blank) digit LEDs must be dark"
        );
        // last digit '7' = segments a,b,c (wire 0,1,2) = LEDs 23,24,25.
        assert_eq!(f[23], Rgb(5, 5, 5));
        assert_eq!(f[24], Rgb(5, 5, 5));
        assert_eq!(f[25], Rgb(5, 5, 5));
    }

    #[test]
    fn value_zero_all_digits_blank() {
        let f = frame(&ax120(), &slot(0, None, Rgb(5, 5, 5)), Rgb(10, 10, 10));
        // "  0" → only last digit lit, showing '0'.
        // '0' = abcdef (wire 0-5), LEDs 23-28.
        assert_eq!(f[23], Rgb(5, 5, 5));
        assert_eq!(f[24], Rgb(5, 5, 5));
        assert_eq!(f[25], Rgb(5, 5, 5));
        assert_eq!(f[26], Rgb(5, 5, 5));
        assert_eq!(f[27], Rgb(5, 5, 5));
        assert_eq!(f[28], Rgb(5, 5, 5));
        assert_eq!(f[29], Rgb(0, 0, 0)); // segment g off for '0'
    }

    #[test]
    fn unknown_slot_is_skipped() {
        let mut slots = HashMap::new();
        slots.insert(
            "nonexistent".to_string(),
            SlotValue {
                value: 57,
                unit: None,
                color: Rgb(5, 5, 5),
                indicators: vec![],
            },
        );
        let f = frame(&ax120(), &slots, Rgb(10, 10, 10));
        assert_eq!(f.len(), 30);
        // Only always-on should be lit.
        assert_eq!(f[0], Rgb(10, 10, 10));
        assert_eq!(f[1], Rgb(10, 10, 10));
    }

    // ── threshold_color ────────────────────────────────────────────────────

    #[test]
    fn thresholds() {
        assert_eq!(
            threshold_color(50.0, Rgb(0, 1, 0), Some(70.0), Some(90.0)),
            Rgb(0, 1, 0)
        );
        assert_eq!(
            threshold_color(75.0, Rgb(0, 1, 0), Some(70.0), Some(90.0)),
            Rgb(255, 170, 0)
        );
        assert_eq!(
            threshold_color(95.0, Rgb(0, 1, 0), Some(70.0), Some(90.0)),
            Rgb(255, 40, 40)
        );
    }

    #[test]
    fn threshold_color_no_warn_or_crit() {
        assert_eq!(
            threshold_color(50.0, Rgb(0, 200, 120), None, None),
            Rgb(0, 200, 120)
        );
    }

    #[test]
    fn threshold_color_exact_warn() {
        // Exactly at warn threshold → amber.
        assert_eq!(
            threshold_color(70.0, Rgb(0, 1, 0), Some(70.0), Some(90.0)),
            Rgb(255, 170, 0)
        );
    }

    #[test]
    fn threshold_color_exact_crit() {
        // Exactly at crit threshold → red.
        assert_eq!(
            threshold_color(90.0, Rgb(0, 1, 0), Some(70.0), Some(90.0)),
            Rgb(255, 40, 40)
        );
    }

    #[test]
    fn threshold_color_crit_overrides_warn() {
        // Above both warn and crit → red (crit wins).
        assert_eq!(
            threshold_color(100.0, Rgb(0, 1, 0), Some(70.0), Some(90.0)),
            Rgb(255, 40, 40)
        );
    }

    #[test]
    fn threshold_color_negative_value() {
        // Negative values are still compared against thresholds.
        assert_eq!(
            threshold_color(-10.0, Rgb(0, 1, 0), Some(70.0), Some(90.0)),
            Rgb(0, 1, 0)
        );
    }

    // ── SlotValue ───────────────────────────────────────────────────────────

    #[test]
    fn slot_value_clone_and_debug() {
        let sv = SlotValue {
            value: 57,
            unit: Some("celsius".into()),
            color: Rgb(0, 200, 120),
            indicators: vec!["cpu".into()],
        };
        let _ = format!("{sv:?}"); // does not panic
        // Verify clone produces equivalent data
        let sv2 = sv.clone();
        assert!(sv.value == sv2.value);
        assert!(sv.unit == sv2.unit);
        assert!(sv.color == sv2.color);
    }
}
