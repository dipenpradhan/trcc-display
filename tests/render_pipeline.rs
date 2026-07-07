//! End-to-end render tests against the *real* shipped profiles.

use std::collections::HashMap;
use std::path::Path;

use trcc_display::profile::ProfileSet;
use trcc_display::protocol::{self, Rgb};
use trcc_display::render::{SlotValue, frame};

fn profiles() -> ProfileSet {
    ProfileSet::load_dir(Path::new("config/profiles")).expect("load config/profiles")
}

fn sv(value: i64, unit: &str, color: Rgb) -> SlotValue {
    SlotValue {
        value,
        unit: Some(unit.to_string()),
        color,
        indicators: vec![],
    }
}

// ── Profile loading ─────────────────────────────────────────────────────

#[test]
fn shipped_profiles_load() {
    let names = profiles().names();
    assert!(names.iter().any(|n| n == "AX120_DIGITAL"), "{names:?}");
    assert!(names.iter().any(|n| n == "PA120_DIGITAL"), "{names:?}");
    assert!(names.iter().any(|n| n == "PS120_DIGITAL"), "{names:?}");
}

#[test]
fn pa120_four_value_frame_and_packet() {
    let p = profiles();
    let pa120 = p.by_name("pa120").expect("pa120 by style name");
    assert_eq!(pa120.mask_size, 84);
    assert_eq!(pa120.remap.as_ref().map(Vec::len), Some(84));

    let mut slots = HashMap::new();
    slots.insert("cpu_temp".into(), sv(57, "celsius", Rgb(0, 200, 120)));
    slots.insert("cpu_use".into(), sv(42, "percent", Rgb(0, 160, 220)));
    slots.insert("gpu_temp".into(), sv(63, "celsius", Rgb(0, 200, 120)));
    slots.insert("gpu_use".into(), sv(99, "percent", Rgb(0, 160, 220)));

    let f = frame(pa120, &slots, Rgb(100, 100, 100));
    assert_eq!(f.len(), 84);
    assert!(
        f.iter().any(|&c| c != Rgb(0, 0, 0)),
        "frame should light something"
    );

    let packet = protocol::data_packet(&f);
    assert_eq!(packet.len(), 20 + 84 * 3);
}

#[test]
fn ps120_four_value_frame_and_packet() {
    let p = profiles();
    let ps120 = p.by_name("ps120").expect("ps120 by style name");
    assert_eq!(ps120.mask_size, 93);
    assert_eq!(ps120.remap.as_ref().map(Vec::len), Some(93));

    let mut slots = HashMap::new();
    slots.insert("temp".into(), sv(57, "celsius", Rgb(0, 200, 120)));
    slots.insert("watt".into(), sv(320, "", Rgb(200, 140, 0)));
    slots.insert("mhz".into(), sv(2610, "", Rgb(130, 130, 220)));
    slots.insert("use".into(), sv(88, "percent", Rgb(0, 160, 220)));

    let f = frame(ps120, &slots, Rgb(100, 100, 100));
    assert_eq!(f.len(), 93);
    assert!(f.iter().any(|&c| c != Rgb(0, 0, 0)));
    assert_eq!(protocol::data_packet(&f).len(), 20 + 93 * 3);
}

#[test]
fn ax120_single_value_frame() {
    let p = profiles();
    let ax120 = p.by_name("ax120").expect("ax120");
    assert_eq!(ax120.mask_size, 30);
    assert!(ax120.remap.is_none());

    let mut slots = HashMap::new();
    slots.insert("main".into(), sv(100, "celsius", Rgb(1, 2, 3)));
    let f = frame(ax120, &slots, Rgb(9, 9, 9));
    assert_eq!(f.len(), 30);
    // always-on LEDs 0,1 are the indicator color.
    assert_eq!(f[0], Rgb(9, 9, 9));
    assert_eq!(f[1], Rgb(9, 9, 9));
}

#[test]
fn pa120_pm_lookup() {
    let p = profiles();
    // PA120 claims PM bytes 16..=31; 20 should resolve to it.
    assert_eq!(p.by_pm(20).map(|p| p.name.as_str()), Some("PA120_DIGITAL"));
    // AX120 claims 1..=3.
    assert_eq!(p.by_pm(3).map(|p| p.name.as_str()), Some("AX120_DIGITAL"));
    // Phantom Spirit 120 Digital EVO (LF8 wire family) claims PM 48/49.
    assert_eq!(p.by_pm(49).map(|p| p.name.as_str()), Some("PS120_DIGITAL"));
    // 200 is unclaimed.
    assert!(p.by_pm(200).is_none());
}

// ── Edge cases ────────────────────────────────────────────────────────────

#[test]
fn ax120_value_at_limit() {
    let p = profiles();
    let ax120 = p.by_name("ax120").expect("ax120");

    let mut slots = HashMap::new();
    slots.insert("main".into(), sv(999, "celsius", Rgb(255, 0, 0)));
    let f = frame(ax120, &slots, Rgb(10, 10, 10));
    // '9' lights more segments than '1', so this should produce a visible frame.
    assert!(f.iter().any(|&c| c != Rgb(0, 0, 0) && c != Rgb(10, 10, 10)));
}

#[test]
fn pa120_partial_digit_hundreds() {
    let p = profiles();
    let pa120 = p.by_name("pa120").expect("pa120");

    let mut slots = HashMap::new();
    // 142% → partial LEDs light for the leading '1', then "42".
    let slot_color = Rgb(0, 200, 120);
    slots.insert("cpu_use".into(), sv(142, "percent", slot_color));
    let f = frame(pa120, &slots, Rgb(100, 100, 100));
    assert_eq!(f.len(), 84);
    // The remap table changes indices, so check that some LEDs have the slot color
    // (not the indicator color and not black).
    assert!(
        f.iter().any(|&c| c == slot_color),
        "partial LEDs and digit segments should be lit with slot color"
    );
}

#[test]
fn ps120_four_digit_mhz() {
    let p = profiles();
    let ps120 = p.by_name("ps120").expect("ps120");

    let mut slots = HashMap::new();
    // 4-digit field: 2610 MHz
    let slot_color = Rgb(130, 130, 220);
    slots.insert("mhz".into(), sv(2610, "", slot_color));
    let f = frame(ps120, &slots, Rgb(100, 100, 100));
    assert_eq!(f.len(), 93);
    // The remap table changes indices, so check that some LEDs have the slot color.
    assert!(
        f.iter().any(|&c| c == slot_color),
        "MHz digit LEDs should be lit with slot color"
    );
}

#[test]
fn all_slots_empty_still_produces_frame() {
    let p = profiles();
    let ax120 = p.by_name("ax120").expect("ax120");

    let f = frame(ax120, &HashMap::new(), Rgb(10, 10, 10));
    assert_eq!(f.len(), 30);
    // Only always-on LEDs should be lit.
    assert_eq!(f[0], Rgb(10, 10, 10));
    assert_eq!(f[1], Rgb(10, 10, 10));
}
