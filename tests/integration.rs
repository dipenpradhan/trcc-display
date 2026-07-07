//! Full-stack integration tests.
//!
//! These tests exercise the interaction between config loading, profile
//! resolution, metric fetching, rendering, and packet building.

use std::collections::HashMap;
use std::path::Path;

use trcc_display::config::Config;
use trcc_display::profile::ProfileSet;
use trcc_display::protocol::{self, Rgb};
use trcc_display::render::{SlotValue, frame};
use trcc_display::sensors;
use trcc_display::state::{Override, RawOverride, Shared};

// ── Config + Profile ──────────────────────────────────────────────────────

#[test]
fn config_loads_from_file() {
    let cfg = Config::load(Path::new("config/config.sensors.json")).expect("load sensors config");
    assert_eq!(cfg.source.kind, "sensors");
    assert!(!cfg.api.enabled);
    assert!(!cfg.tiles.is_empty());
}

#[test]
fn profiles_load_and_resolve() {
    let set = ProfileSet::load_dir(Path::new("config/profiles")).expect("load profiles");
    assert!(set.by_name("ax120").is_some());
    assert!(set.by_name("pa120").is_some());
    assert!(set.by_name("ps120").is_some());
}

#[test]
fn all_profiles_have_valid_geometry() {
    let set = ProfileSet::load_dir(Path::new("config/profiles")).expect("load profiles");
    for name in set.names() {
        let p = set.by_name(&name).unwrap();
        assert!(p.mask_size > 0, "profile {name} has mask_size 0");
        assert!(!p.slots.is_empty(), "profile {name} has no slots");
        for (slot_name, slot) in &p.slots {
            assert!(
                !slot.digits.is_empty(),
                "profile {name} slot {slot_name} has no digits"
            );
            for digit in &slot.digits {
                assert_eq!(
                    digit.len(),
                    7,
                    "profile {name} slot {slot_name} digit has {len} LEDs",
                    len = digit.len()
                );
            }
        }
    }
}

// ── Full pipeline: config → profile → render → packet ────────────────────

#[test]
fn full_pipeline_ax120() {
    // Load config.
    let cfg = Config::load(Path::new("config/config.sensors.json")).expect("load config");

    // Load profiles.
    let set = ProfileSet::load_dir(Path::new(&cfg.profile.dir)).expect("load profiles");

    // Resolve profile (use AX120 for this test).
    let profile = set.by_name("ax120").expect("ax120 profile");

    // Simulate a tile value.
    let mut slots = HashMap::new();
    slots.insert(
        "main".to_string(),
        SlotValue {
            value: 57,
            unit: Some("celsius".into()),
            color: Rgb(0, 200, 120),
            indicators: vec!["cpu".into()],
        },
    );

    // Build frame.
    let indicator = cfg.indicator_color();
    let frame = frame(profile, &slots, indicator);
    assert_eq!(frame.len(), profile.mask_size);

    // Build packet.
    let packet = protocol::data_packet(&frame);
    assert_eq!(packet.len(), 20 + profile.mask_size * 3);

    // Verify packet structure.
    assert_eq!(&packet[0..4], &[0xDA, 0xDB, 0xDC, 0xDD]); // magic
    assert_eq!(packet[12], 2); // CMD_DATA
    let plen = u16::from_le_bytes([packet[16], packet[17]]) as usize;
    assert_eq!(plen, profile.mask_size * 3);
}

#[test]
fn full_pipeline_pa120_four_slots() {
    let set = ProfileSet::load_dir(Path::new("config/profiles")).expect("load profiles");
    let profile = set.by_name("pa120").expect("pa120 profile");

    let mut slots = HashMap::new();
    slots.insert(
        "cpu_temp".into(),
        SlotValue {
            value: 57,
            unit: Some("celsius".into()),
            color: Rgb(0, 200, 120),
            indicators: vec!["cpu".into()],
        },
    );
    slots.insert(
        "cpu_use".into(),
        SlotValue {
            value: 42,
            unit: Some("percent".into()),
            color: Rgb(0, 160, 220),
            indicators: vec!["cpu".into()],
        },
    );
    slots.insert(
        "gpu_temp".into(),
        SlotValue {
            value: 63,
            unit: Some("celsius".into()),
            color: Rgb(0, 200, 120),
            indicators: vec!["gpu".into()],
        },
    );
    slots.insert(
        "gpu_use".into(),
        SlotValue {
            value: 99,
            unit: Some("percent".into()),
            color: Rgb(0, 160, 220),
            indicators: vec!["gpu".into()],
        },
    );

    let frame = frame(profile, &slots, Rgb(100, 100, 100));
    assert_eq!(frame.len(), 84);

    let packet = protocol::data_packet(&frame);
    assert_eq!(packet.len(), 20 + 84 * 3);

    // Chunk and verify.
    let chunks = protocol::chunks(&packet);
    assert_eq!(chunks.len(), 5); // ceil(272 / 64)
    for chunk in &chunks {
        assert_eq!(chunk.len(), 64);
    }
}

// ── Shared state lifecycle ───────────────────────────────────────────────

#[test]
fn shared_state_override_lifecycle() {
    let mut shared = Shared::new();

    // Set an override.
    shared.overrides.insert(
        "main".into(),
        Override {
            value: 99,
            unit: None,
            color: Rgb(255, 0, 0),
            expires_at: None,
        },
    );
    assert_eq!(shared.overrides.len(), 1);

    // Clear.
    shared.overrides.clear();
    assert!(shared.overrides.is_empty());
}

#[test]
fn shared_state_raw_override_lifecycle() {
    let mut shared = Shared::new();

    // Set a raw override.
    shared.raw_override = Some(RawOverride {
        colors: vec![Rgb(255, 0, 0); 30],
        expires_at: None,
    });
    assert!(shared.raw_override.is_some());

    // Clear.
    shared.raw_override = None;
    assert!(shared.raw_override.is_none());
}

#[test]
fn shared_state_tracks_values() {
    let mut shared = Shared::new();
    shared.values.insert("cpu".into(), 57.0);
    shared.values.insert("gpu".into(), 72.0);
    shared.errors.insert("fan".into(), "not found".into());

    assert_eq!(shared.values.len(), 2);
    assert_eq!(shared.errors.len(), 1);
    assert_eq!(shared.values.get("cpu"), Some(&57.0));
    assert_eq!(shared.errors.get("fan"), Some(&"not found".into()));
}

// ── Sensors select ────────────────────────────────────────────────────────

#[test]
fn sensors_select_explicit_subfeature() {
    let tree = serde_json::json!({
        "k10temp-pci-00c3": {
            "Tctl": { "temp1_input": 83.125 }
        }
    });
    let v = sensors::select(&tree, "k10temp-pci-00c3/Tctl/temp1_input").unwrap();
    assert_eq!(v, Some(83.125));
}

#[test]
fn sensors_select_feature_only_prefers_input() {
    let tree = serde_json::json!({
        "chip": {
            "vrm": { "temp1_input": 54.0, "temp1_crit": 100.0 }
        }
    });
    let v = sensors::select(&tree, "chip/vrm").unwrap();
    assert_eq!(v, Some(54.0));
}

#[test]
fn sensors_select_missing_chip() {
    let tree = serde_json::json!({});
    let v = sensors::select(&tree, "missing/feature").unwrap();
    assert_eq!(v, None);
}

#[test]
fn sensors_select_invalid_selector() {
    let tree = serde_json::json!({});
    let result = sensors::select(&tree, "noslash");
    assert!(result.is_err());
}

// ── Rgb conversions ───────────────────────────────────────────────────────

#[test]
fn rgb_roundtrip() {
    let original = Rgb(255, 128, 64);
    let arr: [u8; 3] = original.into();
    let back: Rgb = arr.into();
    assert_eq!(back, original);
}

#[test]
fn rgb_default_is_black() {
    assert_eq!(Rgb::default(), Rgb(0, 0, 0));
}

// ── Seven-segment font ────────────────────────────────────────────────────

#[test]
fn seven_seg_all_digits() {
    for digit in '0'..='9' {
        let segs = protocol::seven_seg(digit);
        // Each digit should light at least some segments.
        assert!(
            segs.iter().any(|&b| b),
            "digit '{digit}' should light some segments"
        );
    }
}

#[test]
fn seven_seg_special_chars() {
    assert!(protocol::seven_seg('C').iter().any(|&b| b));
    assert!(protocol::seven_seg('F').iter().any(|&b| b));
    assert!(protocol::seven_seg('H').iter().any(|&b| b));
    assert!(protocol::seven_seg('G').iter().any(|&b| b));
    assert!(!protocol::seven_seg(' ').iter().any(|&b| b));
}

// ── Protocol packet roundtrip ─────────────────────────────────────────────

#[test]
fn packet_roundtrip_small() {
    let colors = vec![Rgb(255, 0, 0), Rgb(0, 255, 0), Rgb(0, 0, 255)];
    let packet = protocol::data_packet(&colors);
    let chunks = protocol::chunks(&packet);
    assert_eq!(chunks.len(), 1); // 20 + 9 = 29 bytes fits in one 64-byte report

    // Reassemble.
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled[..packet.len()], packet);
}

#[test]
fn packet_roundtrip_large() {
    // 84 LEDs (PA120) → 20 + 252 = 272 bytes → 5 chunks.
    let colors = vec![Rgb(100, 150, 200); 84];
    let packet = protocol::data_packet(&colors);
    assert_eq!(packet.len(), 272);

    let chunks = protocol::chunks(&packet);
    assert_eq!(chunks.len(), 5);

    // Reassemble and verify.
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    // Trim padding.
    reassembled.truncate(packet.len());
    assert_eq!(reassembled, packet);
}
