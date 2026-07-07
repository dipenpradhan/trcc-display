//! Device profiles — the per-model LED geometry, loaded from JSON.
//!
//! A profile is pure *geometry*: how many LEDs, which LED indices form each
//! digit of each named slot, where the unit/indicator LEDs are, and the
//! optional wire-remap table. What to *show* comes from [`crate::config`]
//! tiles, which reference slots by name. This split is what makes the driver
//! model-agnostic: add a new cooler by dropping a JSON file in `profiles/`.
//!
//! # Adding a new cooler
//!
//! 1. Run `trcc-display test-pattern --mode walk` to map physical LEDs.
//! 2. Write a JSON file matching the schema below.
//! 3. Place it in the `profile.dir` directory.
//! 4. The driver auto-discovers it on startup.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// One renderable digit field within a profile (e.g. `gpu_temp`).
///
/// # Fields
///
/// * `digits` — per-digit LED indices, most-significant digit first. Each
///   inner array is 7 indices in segment order `a..g`.
/// * `partial` — optional two LEDs forming a leading `1` for values ≥ 100.
/// * `unit` — optional unit-marker LEDs keyed by unit name.
/// * `on` — LEDs lit whenever this slot is active.
#[derive(Debug, Clone, Deserialize)]
pub struct Slot {
    /// Per-digit LED indices, most-significant digit first. Each inner array is
    /// 7 indices in segment order `a..g` (see [`crate::protocol::WIRE_ORDER`]).
    pub digits: Vec<Vec<usize>>,
    /// Optional two LEDs forming a leading `1` for values ≥ 100 (hundreds).
    #[serde(default)]
    pub partial: Option<Vec<usize>>,
    /// Optional unit-marker LEDs keyed by unit name (`celsius`/`fahrenheit`/`percent`).
    #[serde(default)]
    pub unit: HashMap<String, usize>,
    /// LEDs lit whenever this slot is active (extra indicators).
    #[serde(default)]
    pub on: Vec<usize>,
}

/// A device model: geometry + wire remap.
///
/// # Fields
///
/// * `name` — human-readable name (e.g. `"AX120_DIGITAL"`).
/// * `style` — shorthand for profile lookup (e.g. `"ax120"`).
/// * `pm_bytes` — handshake `PM` bytes that select this profile.
/// * `mask_size` — total LED count (packet size = `mask_size * 3`).
/// * `remap` — optional wire-remap table.
/// * `always_on` — LEDs always lit (static decoration).
/// * `indicators` — named indicator LED groups.
/// * `slots` — named digit fields.
#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    /// Human-readable name.
    pub name: String,
    /// Shorthand for profile lookup.
    #[serde(default)]
    pub style: String,
    /// Handshake `PM` bytes that select this profile.
    #[serde(default)]
    pub pm_bytes: Vec<u8>,
    /// Total LED count (packet size = `mask_size * 3`).
    pub mask_size: usize,
    /// Optional wire-remap: `physical[i] = logical[remap[i]]`. `None` = identity.
    #[serde(default)]
    pub remap: Option<Vec<usize>>,
    /// LEDs always lit (unit frames, static decoration).
    #[serde(default)]
    pub always_on: Vec<usize>,
    /// Named indicator LED groups a tile may request (e.g. `cpu`, `gpu`).
    #[serde(default)]
    pub indicators: HashMap<String, Vec<usize>>,
    /// Named digit fields.
    pub slots: HashMap<String, Slot>,
}

impl Profile {
    /// Load a single profile JSON file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, or validated.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading profile {}", path.display()))?;
        let p: Profile = serde_json::from_str(&text)
            .with_context(|| format!("parsing profile {}", path.display()))?;
        p.validate()
            .with_context(|| format!("validating profile {}", path.display()))?;
        Ok(p)
    }

    /// Reject indices that fall outside `mask_size` — a typo'd profile should
    /// fail loudly at load, not silently corrupt frames.
    fn validate(&self) -> Result<()> {
        let n = self.mask_size;
        let check = |i: usize, what: &str| -> Result<()> {
            anyhow::ensure!(i < n, "{what}: LED index {i} >= mask_size {n}");
            Ok(())
        };
        for &i in &self.always_on {
            check(i, "always_on")?;
        }
        for leds in self.indicators.values() {
            for &i in leds {
                check(i, "indicator")?;
            }
        }
        for (name, slot) in &self.slots {
            for d in &slot.digits {
                anyhow::ensure!(
                    d.len() == 7,
                    "slot {name}: each digit needs 7 LEDs, got {}",
                    d.len()
                );
                for &i in d {
                    check(i, &format!("slot {name} digit"))?;
                }
            }
            if let Some(p) = &slot.partial {
                for &i in p {
                    check(i, &format!("slot {name} partial"))?;
                }
            }
            for &i in slot.unit.values() {
                check(i, &format!("slot {name} unit"))?;
            }
            for &i in &slot.on {
                check(i, &format!("slot {name} on"))?;
            }
        }
        if let Some(remap) = &self.remap {
            anyhow::ensure!(
                remap.len() == n,
                "remap length {} != mask_size {n}",
                remap.len()
            );
        }
        Ok(())
    }

    /// Apply the wire-remap: given `logical` colors, return physical-order
    /// colors ready for the packet. Identity when no table is present.
    ///
    /// # Arguments
    ///
    /// * `logical` — colors in logical LED order (indices 0..mask_size).
    ///
    /// # Returns
    ///
    /// Colors in physical LED order (for wire transmission).
    pub fn to_physical(&self, logical: &[crate::protocol::Rgb]) -> Vec<crate::protocol::Rgb> {
        match &self.remap {
            None => logical.to_vec(),
            Some(table) => table
                .iter()
                .map(|&idx| {
                    logical
                        .get(idx)
                        .copied()
                        .unwrap_or(crate::protocol::Rgb(0, 0, 0))
                })
                .collect(),
        }
    }
}

/// A directory of profiles, selectable by name or by handshake PM byte.
///
/// Profiles are loaded from JSON files in a directory. The driver auto-discovers
/// new profiles on startup by scanning the `profile.dir` directory.
#[derive(Debug, Clone)]
pub struct ProfileSet {
    profiles: Vec<Profile>,
}

impl ProfileSet {
    /// Load every `*.json` file in `dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be read or no profiles are found.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut profiles = Vec::new();
        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("reading profile dir {}", dir.display()))?;
        for entry in entries {
            let path = entry?.path();
            // Skip dotfiles (e.g. macOS `._*` AppleDouble sidecars) and non-JSON.
            let is_dotfile = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if !is_dotfile && path.extension().and_then(|e| e.to_str()) == Some("json") {
                profiles.push(Profile::load(&path)?);
            }
        }
        anyhow::ensure!(
            !profiles.is_empty(),
            "no profiles found in {}",
            dir.display()
        );
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self { profiles })
    }

    /// Find a profile whose file stem or `name` matches (case-insensitive).
    ///
    /// # Arguments
    ///
    /// * `name` — the profile name or style to look up.
    ///
    /// # Returns
    ///
    /// The matching profile, or `None`.
    pub fn by_name(&self, name: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name) || p.style.eq_ignore_ascii_case(name))
    }

    /// Find the profile that claims this handshake `PM` byte.
    ///
    /// # Arguments
    ///
    /// * `pm` — the PM byte from the device handshake.
    ///
    /// # Returns
    ///
    /// The matching profile, or `None`.
    pub fn by_pm(&self, pm: u8) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.pm_bytes.contains(&pm))
    }

    /// Return all profile names.
    pub fn names(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Rgb;

    fn parse(json: serde_json::Value) -> Result<Profile> {
        let p: Profile = serde_json::from_value(json)?;
        p.validate()?;
        Ok(p)
    }

    // ── Profile validation ──────────────────────────────────────────────────

    #[test]
    fn valid_profile_loads() {
        let p = parse(serde_json::json!({
            "name": "T", "mask_size": 10,
            "slots": { "main": { "digits": [[0,1,2,3,4,5,6]] } }
        }))
        .unwrap();
        assert_eq!(p.mask_size, 10);
        assert!(p.slots.contains_key("main"));
    }

    #[test]
    fn out_of_range_index_rejected() {
        let err = parse(serde_json::json!({
            "name": "T", "mask_size": 5,
            "slots": { "main": { "digits": [[0,1,2,3,4,5,9]] } } // 5 and 9 are >= 5
        }))
        .unwrap_err();
        assert!(err.to_string().contains(">= mask_size 5"), "got: {err}");
    }

    #[test]
    fn digit_must_be_seven_leds() {
        let err = parse(serde_json::json!({
            "name": "T", "mask_size": 10,
            "slots": { "main": { "digits": [[0,1,2]] } }
        }))
        .unwrap_err();
        assert!(err.to_string().contains("7 LEDs"), "got: {err}");
    }

    #[test]
    fn remap_length_must_match() {
        let err = parse(serde_json::json!({
            "name": "T", "mask_size": 3, "remap": [0, 1],
            "slots": {}
        }))
        .unwrap_err();
        assert!(err.to_string().contains("remap length"), "got: {err}");
    }

    #[test]
    fn to_physical_identity_and_remap() {
        use crate::protocol::Rgb;
        let identity = parse(serde_json::json!({
            "name": "I", "mask_size": 3, "slots": {}
        }))
        .unwrap();
        let logical = [Rgb(1, 0, 0), Rgb(2, 0, 0), Rgb(3, 0, 0)];
        assert_eq!(identity.to_physical(&logical), logical);

        // remap physical[i] = logical[remap[i]]; reverse order here.
        let rev = parse(serde_json::json!({
            "name": "R", "mask_size": 3, "remap": [2, 1, 0], "slots": {}
        }))
        .unwrap();
        assert_eq!(
            rev.to_physical(&logical),
            vec![Rgb(3, 0, 0), Rgb(2, 0, 0), Rgb(1, 0, 0)]
        );
    }

    #[test]
    fn to_physical_identity_no_remap() {
        let p = parse(serde_json::json!({
            "name": "I", "mask_size": 2, "slots": {}
        }))
        .unwrap();
        let logical = vec![Rgb(10, 20, 30), Rgb(40, 50, 60)];
        let physical = p.to_physical(&logical);
        assert_eq!(physical, logical);
    }

    #[test]
    fn to_physical_remap_out_of_bounds_defaults_to_black() {
        let p = parse(serde_json::json!({
            "name": "R", "mask_size": 2, "remap": [0, 5], "slots": {}
        }))
        .unwrap();
        let logical = vec![Rgb(255, 0, 0)];
        let physical = p.to_physical(&logical);
        assert_eq!(physical.len(), 2);
        assert_eq!(physical[0], Rgb(255, 0, 0));
        assert_eq!(physical[1], Rgb(0, 0, 0)); // index 5 is out of bounds
    }

    #[test]
    fn profile_validation_always_on() {
        let err = parse(serde_json::json!({
            "name": "T", "mask_size": 5, "always_on": [0, 10],
            "slots": {}
        }))
        .unwrap_err();
        assert!(err.to_string().contains(">= mask_size 5"), "got: {err}");
    }

    #[test]
    fn profile_validation_indicators() {
        let err = parse(serde_json::json!({
            "name": "T", "mask_size": 5,
            "indicators": { "cpu": [0, 99] },
            "slots": {}
        }))
        .unwrap_err();
        assert!(err.to_string().contains(">= mask_size 5"), "got: {err}");
    }

    // ── ProfileSet ──────────────────────────────────────────────────────────

    #[test]
    fn profile_set_loads_all_profiles() {
        let set = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();
        let names = set.names();
        assert!(names.contains(&"AX120_DIGITAL".into()));
        assert!(names.contains(&"PA120_DIGITAL".into()));
        assert!(names.contains(&"PS120_DIGITAL".into()));
    }

    #[test]
    fn profile_set_by_name_case_insensitive() {
        let set = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();
        assert!(set.by_name("ax120").is_some());
        assert!(set.by_name("AX120").is_some());
        assert!(set.by_name("Ax120").is_some());
        assert!(set.by_name("pa120").is_some());
        assert!(set.by_name("ps120").is_some());
    }

    #[test]
    fn profile_set_by_pm() {
        let set = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();
        // AX120 claims PM 1, 2, 3
        assert!(set.by_pm(1).is_some());
        assert!(set.by_pm(2).is_some());
        assert!(set.by_pm(3).is_some());
        // PA120 claims PM 16..31
        assert!(set.by_pm(16).is_some());
        assert!(set.by_pm(31).is_some());
        // PS120 claims PM 48, 49
        assert!(set.by_pm(48).is_some());
        assert!(set.by_pm(49).is_some());
    }

    #[test]
    fn profile_set_by_pm_not_found() {
        let set = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();
        assert!(set.by_pm(200).is_none());
    }

    #[test]
    fn profile_set_by_name_not_found() {
        let set = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();
        assert!(set.by_name("nonexistent").is_none());
    }

    #[test]
    fn profile_set_names_sorted() {
        let set = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();
        let names = set.names();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
