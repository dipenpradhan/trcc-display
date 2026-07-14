//! Runtime state shared between the USB worker, the engine, and the REST API.

use std::collections::HashMap;
use std::time::Instant;

use crate::protocol::Rgb;
use serde::Serialize;

/// A REST-set value that takes over one slot until it expires.
///
/// Set via `POST /display/value` and honored by the engine until the
/// optional TTL lapses. A `None` TTL means the override lives until
/// `POST /display/clear` or another override replaces it.
#[derive(Debug, Clone)]
pub struct Override {
    /// The integer value to display (e.g. `57` for 57 °C).
    pub value: i64,
    /// Unit-marker key (`"celsius"`, `"percent"`, …).
    pub unit: Option<String>,
    /// Color for this slot's lit LEDs.
    pub color: Rgb,
    /// When the override expires, if any.
    pub expires_at: Option<Instant>,
}

/// A REST-set full frame (logical colors) that takes over the whole display.
///
/// Set via `POST /display/raw`. Takes precedence over all tiles and slot
/// overrides until it expires.
#[derive(Debug, Clone)]
pub struct RawOverride {
    /// Logical-order RGB colors (one per LED, `profile.mask_size` total).
    pub colors: Vec<Rgb>,
    /// When the override expires, if any.
    pub expires_at: Option<Instant>,
}

/// Central runtime state bucket.
///
/// Shared via `Arc<Mutex<Self>>` between the engine loop, the USB worker
/// thread, and the REST API handlers. All fields are plain data — the mutex
/// is only used for synchronization, not protection against poisoning
/// (see [`crate::util::lock_arc`]).
#[derive(Debug, Default)]
pub struct Shared {
    /// `true` while the USB device is connected and the handshake succeeded.
    pub connected: bool,
    /// Handshake PM byte (device family).
    pub pm: Option<u8>,
    /// Handshake SUB byte (wire-remap sub-variant).
    pub sub: Option<u8>,
    /// Resolved profile name (after PM lookup or force).
    pub profile_name: Option<String>,
    /// Active metric source (`"prometheus"` or `"sensors"`).
    pub source_kind: String,
    /// Latest value per tile name (from the last successful fetch).
    pub values: HashMap<String, f64>,
    /// Latest per-tile error string (cleared on next successful fetch).
    pub errors: HashMap<String, String>,
    /// Slot-level overrides set via the REST API.
    pub overrides: HashMap<String, Override>,
    /// Full-frame override set via the REST API.
    pub raw_override: Option<RawOverride>,
    /// Most recent error message from the USB worker or engine.
    pub last_error: Option<String>,
    /// When the last frame was sent to the display.
    pub last_frame_at: Option<Instant>,
    /// Cumulative count of frames successfully sent.
    pub frames_sent: u64,
    /// Monotonic counter incremented each render cycle, regardless of USB state.
    /// Used by the web preview to detect frame changes.
    pub preview_generation: u64,
    /// Snapshot of the latest rendered frame, for the web preview.
    pub preview_frame: Option<PreviewFrame>,
}

/// A rendered frame snapshot, serializable for the REST preview endpoint.
///
/// Stored in [`Shared`] after each engine render cycle. The web preview page
/// polls this via `GET /preview/frame`.
#[derive(Debug, Clone, Serialize)]
pub struct PreviewFrame {
    /// Monotonic counter incremented each frame; clients skip re-renders
    /// when the generation hasn't changed.
    pub generation: u64,
    /// Active profile name.
    pub profile: String,
    /// Physical-order RGB colors, one per LED.
    pub leds: Vec<[u8; 3]>,
    /// Raw metric values per zone, for display as plain text.
    pub values: HashMap<String, f64>,
}

impl Shared {
    /// Create a fresh empty state.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_new_is_defaults() {
        let s = Shared::new();
        assert!(!s.connected);
        assert!(s.pm.is_none());
        assert!(s.sub.is_none());
        assert!(s.profile_name.is_none());
        assert_eq!(s.source_kind, "");
        assert!(s.values.is_empty());
        assert!(s.errors.is_empty());
        assert!(s.overrides.is_empty());
        assert!(s.raw_override.is_none());
        assert!(s.last_error.is_none());
        assert!(s.last_frame_at.is_none());
        assert_eq!(s.frames_sent, 0);
        assert_eq!(s.preview_generation, 0);
        assert!(s.preview_frame.is_none());
    }

    #[test]
    fn shared_is_default() {
        let s = Shared::default();
        let n = Shared::new();
        // Both should have the same initial state
        assert!(s.connected == n.connected);
        assert!(s.values.is_empty() && n.values.is_empty());
        assert!(s.overrides.is_empty() && n.overrides.is_empty());
        assert!(s.raw_override.is_none() && n.raw_override.is_none());
    }

    #[test]
    fn override_clone_and_debug() {
        let o = Override {
            value: 57,
            unit: Some("celsius".into()),
            color: Rgb(0, 200, 120),
            expires_at: None,
        };
        let _ = format!("{o:?}"); // does not panic
        // Verify clone produces equivalent data
        let o2 = o.clone();
        assert!(o.value == o2.value);
        assert!(o.unit == o2.unit);
        assert!(o.color == o2.color);
    }

    #[test]
    fn raw_override_with_colors() {
        let r = RawOverride {
            colors: vec![Rgb(255, 0, 0), Rgb(0, 255, 0), Rgb(0, 0, 255)],
            expires_at: None,
        };
        assert_eq!(r.colors.len(), 3);
        assert!(r.expires_at.is_none());
    }

    #[test]
    fn shared_tracks_values() {
        let mut s = Shared::new();
        s.values.insert("gpu".into(), 72.5);
        assert_eq!(s.values.get("gpu"), Some(&72.5));
        s.errors.insert("gpu".into(), "timeout".into());
        assert_eq!(s.errors.get("gpu"), Some(&"timeout".into()));
    }

    #[test]
    fn shared_override_lifecycle() {
        let mut s = Shared::new();
        assert!(s.overrides.is_empty());

        s.overrides.insert(
            "main".into(),
            Override {
                value: 99,
                unit: None,
                color: Rgb(255, 0, 0),
                expires_at: None,
            },
        );
        assert_eq!(s.overrides.len(), 1);

        s.overrides.clear();
        assert!(s.overrides.is_empty());
    }

    #[test]
    fn shared_raw_override_set_and_clear() {
        let mut s = Shared::new();
        s.raw_override = Some(RawOverride {
            colors: vec![Rgb(0, 0, 0); 30],
            expires_at: None,
        });
        assert!(s.raw_override.is_some());

        s.raw_override = None;
        assert!(s.raw_override.is_none());
    }
}
