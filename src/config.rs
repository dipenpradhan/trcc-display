//! JSON configuration (`config.json`). Everything the operator tunes lives here;
//! device *geometry* lives in [`crate::profile`]. Unknown keys (e.g. `$comment`)
//! are ignored, so the file can be self-documenting.
//!
//! # Design
//!
//! All config is a single JSON file. The structure mirrors the driver's
//! runtime configuration: USB settings, profile selection, metric source,
//! REST API toggle, render cadence, and the tile array (what to display).
//!
//! Unknown keys are silently ignored, which lets operators document their
//! config files with `$comment` fields without breaking the parser.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::protocol::Rgb;

/// Top-level configuration loaded from the JSON file.
///
/// # Fields
///
/// * `usb` — device identification (VID/PID, interface).
/// * `profile` — profile directory and optional force override.
/// * `source` — metric backend (`"prometheus"` or `"sensors"`).
/// * `prometheus` — Prometheus server URL and timeout.
/// * `api` — REST API toggle and bind address.
/// * `render` — frame cadence, rotation, temperature unit.
/// * `tiles` — array of display tiles (what to show).
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// USB device identification.
    pub usb: UsbCfg,
    /// Profile selection.
    pub profile: ProfileCfg,
    /// Where tile values come from.
    #[serde(default)]
    pub source: SourceCfg,
    /// Prometheus server settings.
    #[serde(default)]
    pub prometheus: PrometheusCfg,
    /// REST API settings.
    #[serde(default)]
    pub api: ApiCfg,
    /// Render cadence and display options.
    pub render: RenderCfg,
    /// Display tiles (what to show on the cooler).
    pub tiles: Vec<Tile>,
}

/// Which backend feeds tile values.
///
/// * `"prometheus"` — tile `query` is PromQL.
/// * `"sensors"` — tile `query` is `chip/feature[/subfeature]` into `sensors -j`.
#[derive(Debug, Clone, Deserialize)]
pub struct SourceCfg {
    /// Backend kind: `"prometheus"` or `"sensors"`.
    #[serde(default = "default_source_kind")]
    pub kind: String,
    /// Command to run for the sensors source (default: `["sensors", "-j"]`).
    #[serde(default = "default_sensors_cmd")]
    pub sensors_command: Vec<String>,
}

impl Default for SourceCfg {
    fn default() -> Self {
        Self {
            kind: default_source_kind(),
            sensors_command: default_sensors_cmd(),
        }
    }
}

/// USB device identification.
///
/// The default `0416:8001` matches all Thermalright "Digital" coolers.
/// The interface is usually `0` (HID).
#[derive(Debug, Clone, Deserialize)]
pub struct UsbCfg {
    /// USB vendor ID (decimal, default 1046 = 0x0416).
    pub vendor_id: u16,
    /// USB product ID (decimal, default 32769 = 0x8001).
    pub product_id: u16,
    /// HID interface number (usually 0).
    #[serde(default)]
    pub interface: u8,
}

/// Profile selection.
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileCfg {
    /// Directory containing profile JSON files.
    pub dir: String,
    /// Skip PM auto-detection and force this profile (file stem or name).
    #[serde(default)]
    pub force: Option<String>,
}

/// Prometheus server settings.
#[derive(Debug, Clone, Deserialize)]
pub struct PrometheusCfg {
    /// Base URL (default: `http://localhost:9090`).
    #[serde(default = "default_prom_url")]
    pub url: String,
    /// Query timeout in seconds (default: 4).
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

impl Default for PrometheusCfg {
    fn default() -> Self {
        Self {
            url: default_prom_url(),
            timeout_seconds: default_timeout(),
        }
    }
}

/// REST API settings.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiCfg {
    /// When `false`, the driver runs fully headless (no HTTP listener).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Bind address (default: `0.0.0.0:9110`).
    #[serde(default = "default_bind")]
    pub bind: String,
    /// When `true`, serves a live LED preview page at `/`.
    #[serde(default)]
    pub preview_enabled: bool,
}

impl Default for ApiCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: default_bind(),
            preview_enabled: false,
        }
    }
}

/// Render cadence and display options.
#[derive(Debug, Clone, Deserialize)]
pub struct RenderCfg {
    /// How often to re-query the metric source (seconds, default 5).
    #[serde(default = "default_refresh")]
    pub refresh_seconds: u64,
    /// When several tiles share a slot, seconds per tile (default 3).
    #[serde(default = "default_rotate")]
    pub rotate_seconds: u64,
    /// Frame cadence in milliseconds (default 250).
    #[serde(default = "default_tick")]
    pub tick_ms: u64,
    /// Temperature unit for display (`"C"` or `"F"`).
    #[serde(default = "default_unit")]
    pub temp_unit: String,
    /// `[r, g, b]` for always-on / unit LEDs.
    #[serde(default = "default_indicator")]
    pub indicator_color: [u8; 3],
}

/// One thing to display. Tiles that share a `slot` are rotated through it.
///
/// # Example
///
/// ```json
/// {
///     "name": "gpu_temp",
///     "slot": "gpu_temp",
///     "query": "max(DCGM_FI_DEV_GPU_TEMP)",
///     "unit": "celsius",
///     "color": [0, 200, 120],
///     "warn": 75,
///     "crit": 84,
///     "indicators": ["gpu"]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tile {
    /// Unique name for this tile (used in status/error reporting).
    pub name: String,
    /// Slot to display in (profile `slots` key).
    pub slot: String,
    /// Query expression (PromQL or `chip/feature`).
    pub query: String,
    /// Unit marker to light: `"celsius"`, `"fahrenheit"`, `"percent"`.
    #[serde(default)]
    pub unit: Option<String>,
    /// Base `[r, g, b]` color (default `[0, 200, 120]`).
    #[serde(default = "default_color")]
    pub color: [u8; 3],
    /// Amber threshold (at or above this value, color becomes amber).
    #[serde(default)]
    pub warn: Option<f64>,
    /// Red threshold (at or above this value, color becomes red).
    #[serde(default)]
    pub crit: Option<f64>,
    /// Indicator groups (profile `indicators` keys) to light for this tile.
    #[serde(default)]
    pub indicators: Vec<String>,
}

impl Tile {
    /// Return the color as an [`Rgb`] tuple.
    pub fn color_rgb(&self) -> Rgb {
        Rgb(self.color[0], self.color[1], self.color[2])
    }
}

impl Config {
    /// Load a config file from the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config = serde_json::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(cfg)
    }

    /// Return the indicator color as an [`Rgb`] tuple.
    pub fn indicator_color(&self) -> Rgb {
        let c = self.render.indicator_color;
        Rgb(c[0], c[1], c[2])
    }
}

fn default_timeout() -> u64 {
    4
}
fn default_true() -> bool {
    true
}
fn default_bind() -> String {
    "0.0.0.0:9110".into()
}
fn default_prom_url() -> String {
    "http://localhost:9090".into()
}
fn default_source_kind() -> String {
    "prometheus".into()
}
fn default_sensors_cmd() -> Vec<String> {
    vec!["sensors".into(), "-j".into()]
}
fn default_refresh() -> u64 {
    5
}
fn default_rotate() -> u64 {
    3
}
fn default_tick() -> u64 {
    250
}
fn default_unit() -> String {
    "C".into()
}
fn default_indicator() -> [u8; 3] {
    [120, 120, 130]
}
fn default_color() -> [u8; 3] {
    [0, 200, 120]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SourceCfg defaults ─────────────────────────────────────────────────

    #[test]
    fn source_cfg_default_is_prometheus() {
        let s = SourceCfg::default();
        assert_eq!(s.kind, "prometheus");
        assert_eq!(s.sensors_command, vec!["sensors", "-j"]);
    }

    #[test]
    fn source_cfg_from_json_sensors() {
        let json = r#"{"kind": "sensors"}"#;
        let s: SourceCfg = serde_json::from_str(json).unwrap();
        assert_eq!(s.kind, "sensors");
        assert_eq!(s.sensors_command, vec!["sensors", "-j"]);
    }

    #[test]
    fn source_cfg_custom_command() {
        let json = r#"{"kind": "sensors", "sensors_command": ["lm-sensors", "-j", "-a"]}"#;
        let s: SourceCfg = serde_json::from_str(json).unwrap();
        assert_eq!(s.kind, "sensors");
        assert_eq!(s.sensors_command, vec!["lm-sensors", "-j", "-a"]);
    }

    // ── UsbCfg ─────────────────────────────────────────────────────────────

    #[test]
    fn usb_cfg_minimal() {
        let json = r#"{"vendor_id": 1046, "product_id": 32769}"#;
        let u: UsbCfg = serde_json::from_str(json).unwrap();
        assert_eq!(u.vendor_id, 1046);
        assert_eq!(u.product_id, 32769);
        assert_eq!(u.interface, 0); // default
    }

    #[test]
    fn usb_cfg_with_interface() {
        let json = r#"{"vendor_id": 1046, "product_id": 32769, "interface": 1}"#;
        let u: UsbCfg = serde_json::from_str(json).unwrap();
        assert_eq!(u.interface, 1);
    }

    // ── PrometheusCfg defaults ─────────────────────────────────────────────

    #[test]
    fn prometheus_cfg_default() {
        let p = PrometheusCfg::default();
        assert_eq!(p.url, "http://localhost:9090");
        assert_eq!(p.timeout_seconds, 4);
    }

    #[test]
    fn prometheus_cfg_from_json() {
        let json = r#"{"url": "http://prometheus:9090", "timeout_seconds": 10}"#;
        let p: PrometheusCfg = serde_json::from_str(json).unwrap();
        assert_eq!(p.url, "http://prometheus:9090");
        assert_eq!(p.timeout_seconds, 10);
    }

    // ── ApiCfg defaults ────────────────────────────────────────────────────

    #[test]
    fn api_cfg_default() {
        let a = ApiCfg::default();
        assert!(a.enabled);
        assert_eq!(a.bind, "0.0.0.0:9110");
    }

    #[test]
    fn api_cfg_disabled() {
        let json = r#"{"enabled": false}"#;
        let a: ApiCfg = serde_json::from_str(json).unwrap();
        assert!(!a.enabled);
    }

    // ── RenderCfg defaults ─────────────────────────────────────────────────

    #[test]
    fn render_cfg_default() {
        let json = r"{}";
        let r: RenderCfg = serde_json::from_str(json).unwrap();
        assert_eq!(r.refresh_seconds, 5);
        assert_eq!(r.rotate_seconds, 3);
        assert_eq!(r.tick_ms, 250);
        assert_eq!(r.temp_unit, "C");
        assert_eq!(r.indicator_color, [120, 120, 130]);
    }

    #[test]
    fn render_cfg_custom_values() {
        let json = r#"{
            "refresh_seconds": 10,
            "rotate_seconds": 5,
            "tick_ms": 500,
            "temp_unit": "F",
            "indicator_color": [255, 0, 0]
        }"#;
        let r: RenderCfg = serde_json::from_str(json).unwrap();
        assert_eq!(r.refresh_seconds, 10);
        assert_eq!(r.rotate_seconds, 5);
        assert_eq!(r.tick_ms, 500);
        assert_eq!(r.temp_unit, "F");
        assert_eq!(r.indicator_color, [255, 0, 0]);
    }

    // ── Tile ────────────────────────────────────────────────────────────────

    #[test]
    fn tile_minimal() {
        let json = r#"{"name": "t", "slot": "main", "query": "up"}"#;
        let t: Tile = serde_json::from_str(json).unwrap();
        assert_eq!(t.name, "t");
        assert_eq!(t.slot, "main");
        assert_eq!(t.query, "up");
        assert_eq!(t.unit, None);
        assert_eq!(t.color, [0, 200, 120]);
        assert_eq!(t.warn, None);
        assert_eq!(t.crit, None);
        assert!(t.indicators.is_empty());
    }

    #[test]
    fn tile_full() {
        let json = r#"{
            "name": "gpu_temp",
            "slot": "gpu_temp",
            "query": "max(DCGM_FI_DEV_GPU_TEMP)",
            "unit": "celsius",
            "color": [255, 60, 60],
            "warn": 75,
            "crit": 84,
            "indicators": ["gpu"]
        }"#;
        let t: Tile = serde_json::from_str(json).unwrap();
        assert_eq!(t.name, "gpu_temp");
        assert_eq!(t.unit, Some("celsius".into()));
        assert_eq!(t.color, [255, 60, 60]);
        assert_eq!(t.warn, Some(75.0));
        assert_eq!(t.crit, Some(84.0));
        assert_eq!(t.indicators, vec![String::from("gpu")]);
    }

    #[test]
    fn tile_color_rgb() {
        let t: Tile =
            serde_json::from_str(r#"{"name":"t","slot":"s","query":"q","color":[1,2,3]}"#).unwrap();
        assert_eq!(t.color_rgb(), Rgb(1, 2, 3));
    }

    // ── Config ──────────────────────────────────────────────────────────────

    #[test]
    fn parses_minimal_with_defaults() {
        let json = r#"{
            "usb": { "vendor_id": 1046, "product_id": 32769 },
            "profile": { "dir": "config/profiles" },
            "render": { "temp_unit": "C" },
            "tiles": [
                { "name": "t", "slot": "main", "query": "up" }
            ]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        // Defaults kick in for omitted sections.
        assert_eq!(cfg.source.kind, "prometheus");
        assert_eq!(cfg.source.sensors_command, vec!["sensors", "-j"]);
        assert!(cfg.api.enabled);
        assert_eq!(cfg.api.bind, "0.0.0.0:9110");
        assert_eq!(cfg.prometheus.url, "http://localhost:9090");
        assert_eq!(cfg.render.refresh_seconds, 5);
        assert_eq!(cfg.render.indicator_color, [120, 120, 130]);
        // Tile defaults.
        assert_eq!(cfg.tiles[0].color, [0, 200, 120]);
        assert_eq!(cfg.tiles[0].unit, None);
    }

    #[test]
    fn ignores_comment_keys() {
        let json = r#"{
            "$comment": "hi",
            "usb": { "vendor_id": 1, "product_id": 2, "interface": 0 },
            "profile": { "dir": "x", "force": "pa120" },
            "source": { "kind": "sensors" },
            "api": { "enabled": false, "bind": "127.0.0.1:1" },
            "render": {},
            "tiles": []
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.source.kind, "sensors");
        assert_eq!(cfg.profile.force.as_deref(), Some("pa120"));
        assert!(!cfg.api.enabled);
    }

    #[test]
    fn config_indicator_color() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "usb": { "vendor_id": 1, "product_id": 2 },
                "profile": { "dir": "p" },
                "render": { "indicator_color": [255, 128, 0] },
                "tiles": []
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.indicator_color(), Rgb(255, 128, 0));
    }

    #[test]
    fn config_with_multiple_tiles() {
        let json = r#"{
            "usb": { "vendor_id": 1046, "product_id": 32769 },
            "profile": { "dir": "profiles" },
            "render": {},
            "tiles": [
                { "name": "cpu", "slot": "cpu_temp", "query": "node_cpu_temp" },
                { "name": "gpu", "slot": "gpu_temp", "query": "gpu_temp" },
                { "name": "fan", "slot": "cpu_use", "query": "fan_rpm" }
            ]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.tiles.len(), 3);
        assert_eq!(cfg.tiles[0].name, "cpu");
        assert_eq!(cfg.tiles[1].slot, "gpu_temp");
        assert_eq!(cfg.tiles[2].query, "fan_rpm");
    }

    #[test]
    fn config_fahrenheit_unit() {
        let json = r#"{
            "usb": { "vendor_id": 1, "product_id": 2 },
            "profile": { "dir": "p" },
            "render": { "temp_unit": "F" },
            "tiles": []
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.render.temp_unit, "F");
    }
}
