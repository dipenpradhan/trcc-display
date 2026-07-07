//! Local metric source: the JSON emitted by `sensors -j` (lm-sensors).
//!
//! This lets the driver run with **no Prometheus and no REST API** — a
//! self-contained loop that reads the machine's own sensors and paints the
//! cooler. A tile's `query` is a selector into the `sensors -j` tree:
//!
//! ```text
//!   chip/feature/subfeature   e.g.  k10temp-pci-00c3/Tctl/temp1_input
//!   chip/feature              e.g.  amdgpu-pci-1200/edge   (first *_input reading)
//! ```
//!
//! Feature names may themselves contain `/` (e.g. `PECI/TSI Agent 0
//! Calibration`); the selector resolves the longest exact feature match first,
//! so both forms work.

use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

/// Reads sensor JSON by shelling out to `sensors -j` (or a configured command).
///
/// # Arguments
///
/// * `command` — argv, e.g. `["sensors", "-j"]`.
#[derive(Debug, Clone)]
pub struct SensorsSource {
    command: Vec<String>,
}

impl SensorsSource {
    /// Create a new sensors source from an argv.
    ///
    /// # Errors
    ///
    /// Returns an error if the command vector is empty.
    pub fn new(command: Vec<String>) -> Result<Self> {
        anyhow::ensure!(
            !command.is_empty(),
            "source.sensors_command must not be empty"
        );
        Ok(Self { command })
    }

    /// Run the command and parse its stdout as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails or the output is not valid JSON.
    pub fn read(&self) -> Result<Value> {
        let (bin, args) = self
            .command
            .split_first()
            .expect("non-empty (checked in new)");
        let out = Command::new(bin).args(args).output().with_context(|| {
            format!(
                "running `{}` (is lm-sensors installed?)",
                self.command.join(" ")
            )
        })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "`{}` exited with {}: {}",
                self.command.join(" "),
                out.status,
                stderr.trim()
            );
        }
        serde_json::from_slice(&out.stdout).with_context(|| {
            format!(
                "parsing JSON from `{}` (got {} bytes)",
                self.command.join(" "),
                out.stdout.len()
            )
        })
    }
}

/// Resolve a `chip/feature[/subfeature]` selector against a `sensors -j` tree.
///
/// Returns `Ok(None)` when the path doesn't exist (caller decides whether that
/// is fatal); `Err` only for a structurally broken selector.
///
/// # Arguments
///
/// * `root` — the parsed JSON tree from `sensors -j`.
/// * `selector` — the selector string (e.g. `k10temp-pci-00c3/Tctl/temp1_input`).
///
/// # Returns
///
/// `Ok(Some(value))` on success, `Ok(None)` if the path doesn't exist,
/// `Err` if the selector is malformed.
pub fn select(root: &Value, selector: &str) -> Result<Option<f64>> {
    let Some((chip, rest)) = selector.split_once('/') else {
        anyhow::bail!("selector {selector:?} must be chip/feature[/subfeature]");
    };
    let Some(chip_obj) = root.get(chip).and_then(Value::as_object) else {
        return Ok(None); // chip not present this read
    };

    // Longest exact feature match first (handles feature names containing '/').
    if let Some(feat) = chip_obj.get(rest) {
        return Ok(pick_input(feat));
    }
    // Otherwise treat the trailing segment as an explicit subfeature.
    if let Some((feature, sub)) = rest.rsplit_once('/') {
        if let Some(feat) = chip_obj.get(feature).and_then(Value::as_object) {
            return Ok(feat.get(sub).and_then(Value::as_f64));
        }
    }
    Ok(None)
}

/// Pick a representative number from a feature object: prefer a `*_input`
/// subfeature, else the first numeric value.
fn pick_input(feature: &Value) -> Option<f64> {
    let obj = feature.as_object()?;
    if let Some((_, v)) = obj.iter().find(|(k, _)| k.ends_with("_input")) {
        if let Some(n) = v.as_f64() {
            return Some(n);
        }
    }
    obj.values().find_map(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree() -> Value {
        json!({
            "k10temp-pci-00c3": {
                "Adapter": "PCI adapter",
                "Tctl": { "temp1_input": 83.125 },
                "Tccd1": { "temp2_input": 80.875 }
            },
            "corsairpsu-hid-3-9": {
                "psu fan": { "fan1_input": 372.0 },
                "vrm temp": { "temp1_input": 54.0, "temp1_crit": 100.0 }
            },
            "nct6799-isa-0290": {
                "PECI/TSI Agent 0 Calibration": { "temp7_input": 72.0 }
            }
        })
    }

    // ── select ──────────────────────────────────────────────────────────────

    #[test]
    fn explicit_subfeature() {
        let v = select(&tree(), "k10temp-pci-00c3/Tctl/temp1_input").unwrap();
        assert_eq!(v, Some(83.125));
    }

    #[test]
    fn feature_only_prefers_input() {
        // vrm temp has both temp1_input and temp1_crit; must pick _input.
        let v = select(&tree(), "corsairpsu-hid-3-9/vrm temp").unwrap();
        assert_eq!(v, Some(54.0));
    }

    #[test]
    fn feature_name_with_slash() {
        let v = select(&tree(), "nct6799-isa-0290/PECI/TSI Agent 0 Calibration").unwrap();
        assert_eq!(v, Some(72.0));
    }

    #[test]
    fn missing_chip_is_none_not_error() {
        assert_eq!(select(&tree(), "nope/whatever").unwrap(), None);
    }

    #[test]
    fn selector_without_slash_errors() {
        assert!(select(&tree(), "justachip").is_err());
    }

    #[test]
    fn missing_feature_is_none() {
        assert_eq!(
            select(&tree(), "k10temp-pci-00c3/nonexistent").unwrap(),
            None
        );
    }

    #[test]
    fn missing_subfeature_is_none() {
        assert_eq!(
            select(&tree(), "k10temp-pci-00c3/Tctl/nonexistent").unwrap(),
            None
        );
    }

    #[test]
    fn select_fan_speed() {
        let v = select(&tree(), "corsairpsu-hid-3-9/psu fan/fan1_input").unwrap();
        assert_eq!(v, Some(372.0));
    }

    #[test]
    fn select_second_feature() {
        let v = select(&tree(), "k10temp-pci-00c3/Tccd1/temp2_input").unwrap();
        assert_eq!(v, Some(80.875));
    }

    // ── SensorsSource ───────────────────────────────────────────────────────

    #[test]
    fn sensors_source_empty_command_fails() {
        assert!(SensorsSource::new(vec![]).is_err());
    }

    #[test]
    fn sensors_source_valid_command() {
        let src = SensorsSource::new(vec!["sensors".into(), "-j".into()]).unwrap();
        // Don't actually run it (sensors may not be installed).
        drop(src);
    }

    #[test]
    fn sensors_source_clone() {
        let src = SensorsSource::new(vec!["sensors".into(), "-j".into()]).unwrap();
        let _ = src.clone();
    }

    // ── pick_input ─────────────────────────────────────────────────────────

    #[test]
    fn pick_input_prefers_input_over_other() {
        let feature = json!({
            "temp1_input": 50.0,
            "temp1_crit": 100.0,
            "temp1_max": 120.0
        });
        assert_eq!(pick_input(&feature), Some(50.0));
    }

    #[test]
    fn pick_input_no_input_key() {
        let feature = json!({
            "speed": 1000.0
        });
        assert_eq!(pick_input(&feature), Some(1000.0));
    }

    #[test]
    fn pick_input_not_an_object() {
        let feature = json!(42.0);
        assert_eq!(pick_input(&feature), None);
    }

    #[test]
    fn pick_input_empty_object() {
        let feature = json!({});
        assert_eq!(pick_input(&feature), None);
    }
}
