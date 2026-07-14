//! The render loop and shared runtime state.
//!
//! The engine ticks fast (default 250 ms). On each tick it: refreshes tile
//! values from the [`Source`] every `refresh_seconds`, applies any REST
//! overrides, rotates tiles that share a slot, renders a frame, and hands the
//! packet to the USB worker over a bounded channel (dropping a frame if the
//! worker is momentarily busy — the next tick sends a fresh one).
//!
//! # Architecture
//!
//! ```text
//! tick ──▶ refresh_metrics ──▶ apply_overrides ──▶ build_frame ──▶ send_to_usb
//!     │         │                   │                     │
//!     │         └─── every N seconds └─── from REST API    └─── via SyncSender
//!     │                                                 
//!     └─── profile resolution (on PM change)
//! ```

use std::collections::HashMap;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{Config, RenderCfg, Tile};
use crate::profile::{Profile, ProfileSet};
use crate::protocol::{self, Rgb};
use crate::render::{self, SlotValue};
use crate::source::Source;
use crate::state::{PreviewFrame, Shared};
use crate::util::lock_arc;

/// Check whether an override is still alive.
///
/// # Arguments
///
/// * `expires_at` — when the override expires, or `None` for permanent.
/// * `now` — current time.
///
/// # Returns
///
/// `true` if the override is still valid at `now`.
fn alive(expires_at: Option<Instant>, now: Instant) -> bool {
    match expires_at {
        Some(e) => e > now,
        None => true,
    }
}

/// Run the render loop until the USB channel closes.
///
/// This is the main driver loop. It runs on a tokio task and communicates
/// with the USB worker thread via a bounded [`SyncSender`].
///
/// # Arguments
///
/// * `cfg` — shared config (can be hot-reloaded).
/// * `profiles` — loaded device profiles.
/// * `source` — metric source (Prometheus or sensors).
/// * `shared` — shared runtime state.
/// * `tx` — channel to send packets to the USB worker.
pub async fn run(
    cfg: Arc<Mutex<Config>>,
    profiles: Arc<ProfileSet>,
    source: Source,
    shared: Arc<Mutex<Shared>>,
    tx: SyncSender<Vec<u8>>,
) {
    {
        let mut s = lock_arc(&shared);
        s.source_kind = source.kind().to_string();
    }

    let tick_ms = lock_arc(&cfg).render.tick_ms.max(50);
    let mut ticker = tokio::time::interval(Duration::from_millis(tick_ms));
    let start = Instant::now();
    let mut last_refresh: Option<Instant> = None;
    let mut profile: Option<Profile> = None;
    let mut resolved_key: Option<String> = None;

    loop {
        ticker.tick().await;

        let (tiles, render, refresh, force) = {
            let c = lock_arc(&cfg);
            (
                c.tiles.clone(),
                c.render.clone(),
                c.render.refresh_seconds.max(1),
                c.profile.force.clone(),
            )
        };

        // (Re)resolve the profile when the force setting or detected PM changes.
        let pm = lock_arc(&shared).pm;
        let want_key = force.clone().or_else(|| pm.map(|p| format!("pm:{p}")));
        if want_key != resolved_key {
            profile = resolve_profile(&profiles, force.as_deref(), pm);
            resolved_key = want_key;
            let name = profile.as_ref().map(|p| p.name.clone());
            lock_arc(&shared).profile_name = name;
            if let Some(p) = &profile {
                tracing::info!(profile = %p.name, leds = p.mask_size, "using profile");
            }
        }
        let Some(profile) = profile.as_ref() else {
            continue; // no device identity / profile yet
        };

        // Refresh metric values on the configured cadence.
        let due = last_refresh.is_none_or(|t| t.elapsed() >= Duration::from_secs(refresh));
        if due {
            let fetched = source.fetch(&tiles).await;
            let mut s = lock_arc(&shared);
            for f in fetched {
                match f.value {
                    Ok(Some(v)) => {
                        s.values.insert(f.tile.clone(), v);
                        s.errors.remove(&f.tile);
                    }
                    Ok(None) => {
                        s.errors
                            .insert(f.tile, "no data (query matched nothing)".into());
                    }
                    Err(e) => {
                        tracing::warn!(tile = %f.tile, error = %e, "value fetch failed");
                        s.errors.insert(f.tile.clone(), e.clone());
                        s.last_error = Some(e);
                    }
                }
            }
            last_refresh = Some(Instant::now());
        }

        if let Some(packet) = build_frame(profile, &tiles, &render, &shared, start) {
            let preview = build_preview_frame(profile, &tiles, &render, &shared, start);
            match tx.try_send(packet) {
                Ok(()) => {
                    let mut s = lock_arc(&shared);
                    s.frames_sent += 1;
                    s.last_frame_at = Some(Instant::now());
                    s.preview_generation = s.preview_generation.saturating_add(1);
                    s.preview_frame = preview;
                }
                Err(TrySendError::Full(_)) => {
                    tracing::trace!("usb worker busy; dropped a frame");
                    // Still update preview
                    if let Some(p) = preview {
                        let mut s = lock_arc(&shared);
                        s.preview_generation = s.preview_generation.saturating_add(1);
                        s.preview_frame = Some(p);
                    }
                }
                Err(TrySendError::Disconnected(_)) => {
                    // USB worker gone but keep rendering for preview
                    tracing::info!("usb worker gone; running without USB (preview only)");
                    if let Some(p) = preview {
                        let mut s = lock_arc(&shared);
                        s.preview_generation = s.preview_generation.saturating_add(1);
                        s.preview_frame = Some(p);
                    }
                }
            }
        }
    }
}

/// Resolve the active profile from profiles by force name or PM byte.
///
/// # Arguments
///
/// * `profiles` — loaded profile set.
/// * `force` — forced profile name (if set in config).
/// * `pm` — handshake PM byte.
///
/// # Returns
///
/// The matching profile, or `None` if nothing matches.
fn resolve_profile(profiles: &ProfileSet, force: Option<&str>, pm: Option<u8>) -> Option<Profile> {
    if let Some(name) = force {
        return profiles.by_name(name).cloned();
    }
    profiles.by_pm(pm?).cloned()
}

/// Build one frame's packet from current values + overrides. `None` = nothing
/// to show yet (no values, no overrides) — the USB display just holds.
///
/// # Arguments
///
/// * `profile` — active device profile.
/// * `tiles` — tile configuration.
/// * `render` — render settings.
/// * `shared` — shared runtime state (values, overrides).
/// * `start` — when the engine started (for rotation timing).
///
/// # Returns
///
/// A full data packet ready for [`protocol::chunks`], or `None` if there
/// is nothing to display yet.
fn build_frame(
    profile: &Profile,
    tiles: &[Tile],
    render: &RenderCfg,
    shared: &Arc<Mutex<Shared>>,
    start: Instant,
) -> Option<Vec<u8>> {
    let now = Instant::now();
    let indicator = Rgb(
        render.indicator_color[0],
        render.indicator_color[1],
        render.indicator_color[2],
    );

    let mut s = lock_arc(shared);

    // Expire stale overrides.
    s.overrides.retain(|_, o| alive(o.expires_at, now));
    if s.raw_override
        .as_ref()
        .is_some_and(|r| !alive(r.expires_at, now))
    {
        s.raw_override = None;
    }

    // Full-frame override wins outright.
    if let Some(raw) = &s.raw_override {
        let physical = profile.to_physical(&raw.colors);
        return Some(protocol::data_packet(&physical));
    }

    let mut slots: HashMap<String, SlotValue> = HashMap::new();

    // Tiles, grouped by slot; tiles sharing a slot rotate over time.
    let elapsed = start.elapsed().as_secs();
    let rotate = render.rotate_seconds.max(1);
    for (slot, group) in group_by_slot(tiles) {
        if s.overrides.contains_key(&slot) {
            continue; // an override owns this slot this frame
        }
        let idx = usize::try_from(elapsed / rotate).unwrap_or(0) % group.len();
        let tile = group[idx];
        if let Some(&v) = s.values.get(&tile.name) {
            slots.insert(slot, tile_slot_value(tile, v, render));
        }
    }

    // Slot overrides.
    for (slot, o) in &s.overrides {
        slots.insert(
            slot.clone(),
            SlotValue {
                value: o.value,
                unit: o.unit.clone(),
                color: o.color,
                indicators: Vec::new(),
            },
        );
    }
    drop(s);

    if slots.is_empty() {
        return None;
    }
    let frame = render::frame(profile, &slots, indicator);
    Some(protocol::data_packet(&frame))
}

/// Build a frame snapshot for the web preview.
///
/// Mirrors `build_frame` but returns raw RGB colors instead of a packet.
fn build_preview_frame(
    profile: &Profile,
    tiles: &[Tile],
    render: &RenderCfg,
    shared: &Arc<Mutex<Shared>>,
    start: Instant,
) -> Option<PreviewFrame> {
    let indicator = Rgb(
        render.indicator_color[0],
        render.indicator_color[1],
        render.indicator_color[2],
    );
    let now = Instant::now();

    let mut s = lock_arc(shared);
    s.overrides.retain(|_, o| alive(o.expires_at, now));
    if s.raw_override
        .as_ref()
        .is_some_and(|r| !alive(r.expires_at, now))
    {
        s.raw_override = None;
    }

    // Full-frame raw override takes precedence.
    if let Some(raw) = &s.raw_override {
        let physical = profile.to_physical(&raw.colors);
        return Some(PreviewFrame {
            generation: s.frames_sent,
            profile: s.profile_name.clone().unwrap_or_default(),
            leds: physical.into_iter().map(|c| [c.0, c.1, c.2]).collect(),
            values: HashMap::new(),
        });
    }

    let mut slots: HashMap<String, SlotValue> = HashMap::new();
    let elapsed = start.elapsed().as_secs();
    let rotate = render.rotate_seconds.max(1);
    for (slot, group) in group_by_slot(tiles) {
        if s.overrides.contains_key(&slot) {
            continue;
        }
        let idx = usize::try_from(elapsed / rotate).unwrap_or(0) % group.len();
        let tile = group[idx];
        if let Some(&v) = s.values.get(&tile.name) {
            slots.insert(slot, tile_slot_value(tile, v, render));
        }
    }
    for (slot, o) in &s.overrides {
        slots.insert(
            slot.clone(),
            SlotValue {
                value: o.value,
                unit: o.unit.clone(),
                color: o.color,
                indicators: Vec::new(),
            },
        );
    }
    let frame_gen = s.preview_generation + 1;
    let name = s.profile_name.clone();
    drop(s);

    if slots.is_empty() {
        return None;
    }
    let frame = render::frame_logical(profile, &slots, indicator);
    let values: HashMap<String, f64> = slots.iter()
        .map(|(k, sv)| (k.clone(), sv.value as f64))
        .collect();
    Some(PreviewFrame {
        generation: frame_gen,
        profile: name.unwrap_or_default(),
        leds: frame.into_iter().map(|c| [c.0, c.1, c.2]).collect(),
        values,
    })
}

/// Group tiles by their slot, preserving first-seen order for stable rotation.
///
/// # Returns
///
/// A vector of `(slot_name, tiles)` pairs in the order slots first appeared.
fn group_by_slot(tiles: &[Tile]) -> Vec<(String, Vec<&Tile>)> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, Vec<&Tile>> = HashMap::new();
    for t in tiles {
        if !map.contains_key(&t.slot) {
            order.push(t.slot.clone());
        }
        map.entry(t.slot.clone()).or_default().push(t);
    }
    order
        .into_iter()
        .map(|s| {
            let v = map.remove(&s).unwrap_or_default();
            (s, v)
        })
        .collect()
}

/// Convert a tile's raw value into a [`SlotValue`] for rendering.
///
/// Handles unit conversion (Celsius → Fahrenheit) and threshold coloring.
fn tile_slot_value(tile: &Tile, raw: f64, render: &RenderCfg) -> SlotValue {
    let mut value = raw;
    let mut unit = tile.unit.clone();
    // Convert Celsius tiles to Fahrenheit if the display is set to F.
    if render.temp_unit.eq_ignore_ascii_case("F") && unit.as_deref() == Some("celsius") {
        value = value * 9.0 / 5.0 + 32.0;
        unit = Some("fahrenheit".to_string());
    }
    SlotValue {
        value: value.round() as i64,
        unit,
        color: render::threshold_color(raw, tile.color_rgb(), tile.warn, tile.crit),
        indicators: tile.indicators.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileSet;
    use crate::protocol::Rgb;
    use std::time::Duration;

    // ── alive ───────────────────────────────────────────────────────────────

    #[test]
    fn alive_none_is_true() {
        assert!(alive(None, Instant::now()));
    }

    #[test]
    fn alive_future_is_true() {
        let future = Instant::now() + Duration::from_secs(60);
        assert!(alive(Some(future), Instant::now()));
    }

    #[test]
    fn alive_past_is_false() {
        let past = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();
        assert!(!alive(Some(past), Instant::now()));
    }

    #[test]
    fn alive_exact_boundary() {
        // Instant comparison is >, not >=. At exact boundary, alive returns false.
        let now = Instant::now();
        assert!(!alive(Some(now), now));
    }

    // ── group_by_slot ───────────────────────────────────────────────────────

    fn tile(name: &str, slot: &str) -> Tile {
        Tile {
            name: name.into(),
            slot: slot.into(),
            query: "up".into(),
            unit: None,
            color: [0, 200, 120],
            warn: None,
            crit: None,
            indicators: vec![],
        }
    }

    #[test]
    fn group_by_slot_empty() {
        let result = group_by_slot(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn group_by_slot_single() {
        let tiles = vec![tile("t1", "main")];
        let groups = group_by_slot(&tiles);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "main");
        assert_eq!(groups[0].1.len(), 1);
    }

    #[test]
    fn group_by_slot_multiple_slots() {
        let tiles = vec![tile("t1", "cpu_temp"), tile("t2", "gpu_temp")];
        let groups = group_by_slot(&tiles);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "cpu_temp");
        assert_eq!(groups[1].0, "gpu_temp");
    }

    #[test]
    fn group_by_slot_same_slot() {
        let tiles = vec![tile("t1", "main"), tile("t2", "main"), tile("t3", "main")];
        let groups = group_by_slot(&tiles);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "main");
        assert_eq!(groups[0].1.len(), 3);
    }

    #[test]
    fn group_by_slot_mixed() {
        let tiles = vec![
            tile("t1", "main"),
            tile("t2", "cpu_temp"),
            tile("t3", "main"),
            tile("t4", "gpu_temp"),
        ];
        let groups = group_by_slot(&tiles);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].0, "main");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "cpu_temp");
        assert_eq!(groups[1].1.len(), 1);
        assert_eq!(groups[2].0, "gpu_temp");
        assert_eq!(groups[2].1.len(), 1);
    }

    #[test]
    fn group_by_slot_preserves_order() {
        // Slots should appear in first-seen order.
        let tiles = vec![
            tile("t1", "gpu_temp"),
            tile("t2", "cpu_temp"),
            tile("t3", "main"),
        ];
        let groups = group_by_slot(&tiles);
        assert_eq!(groups[0].0, "gpu_temp");
        assert_eq!(groups[1].0, "cpu_temp");
        assert_eq!(groups[2].0, "main");
    }

    // ── tile_slot_value ─────────────────────────────────────────────────────

    fn celsius_render() -> RenderCfg {
        RenderCfg {
            refresh_seconds: 5,
            rotate_seconds: 3,
            tick_ms: 250,
            temp_unit: "C".into(),
            indicator_color: [120, 120, 130],
        }
    }

    fn fahrenheit_render() -> RenderCfg {
        RenderCfg {
            refresh_seconds: 5,
            rotate_seconds: 3,
            tick_ms: 250,
            temp_unit: "F".into(),
            indicator_color: [120, 120, 130],
        }
    }

    #[test]
    fn tile_slot_value_celsius_no_conversion() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "temp".into(),
            unit: Some("celsius".into()),
            color: [0, 200, 120],
            warn: None,
            crit: None,
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, 57.0, &celsius_render());
        assert_eq!(sv.value, 57);
        assert_eq!(sv.unit, Some("celsius".into()));
        assert_eq!(sv.color, Rgb(0, 200, 120));
    }

    #[test]
    fn tile_slot_value_fahrenheit_conversion() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "temp".into(),
            unit: Some("celsius".into()),
            color: [0, 200, 120],
            warn: None,
            crit: None,
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, 100.0, &fahrenheit_render());
        // 100C = 212F
        assert_eq!(sv.value, 212);
        assert_eq!(sv.unit, Some("fahrenheit".into()));
    }

    #[test]
    fn tile_slot_value_percent_no_conversion() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "load".into(),
            unit: Some("percent".into()),
            color: [0, 200, 120],
            warn: None,
            crit: None,
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, 75.0, &fahrenheit_render());
        // Percent should not be converted.
        assert_eq!(sv.value, 75);
        assert_eq!(sv.unit, Some("percent".into()));
    }

    #[test]
    fn tile_slot_value_warn_threshold() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "temp".into(),
            unit: None,
            color: [0, 200, 120],
            warn: Some(70.0),
            crit: Some(90.0),
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, 75.0, &celsius_render());
        // 75 >= 70 (warn), so color should be amber.
        assert_eq!(sv.color, Rgb(255, 170, 0));
    }

    #[test]
    fn tile_slot_value_crit_threshold() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "temp".into(),
            unit: None,
            color: [0, 200, 120],
            warn: Some(70.0),
            crit: Some(90.0),
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, 95.0, &celsius_render());
        // 95 >= 90 (crit), so color should be red.
        assert_eq!(sv.color, Rgb(255, 40, 40));
    }

    #[test]
    fn tile_slot_value_base_color() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "temp".into(),
            unit: None,
            color: [0, 200, 120],
            warn: Some(70.0),
            crit: Some(90.0),
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, 50.0, &celsius_render());
        // 50 < 70, so base color.
        assert_eq!(sv.color, Rgb(0, 200, 120));
    }

    #[test]
    fn tile_slot_value_rounds() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "temp".into(),
            unit: None,
            color: [0, 0, 0],
            warn: None,
            crit: None,
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, 57.6, &celsius_render());
        assert_eq!(sv.value, 58);
    }

    #[test]
    fn tile_slot_value_negative_clamps() {
        let tile = Tile {
            name: "t".into(),
            slot: "main".into(),
            query: "temp".into(),
            unit: None,
            color: [0, 0, 0],
            warn: None,
            crit: None,
            indicators: vec![],
        };
        let sv = tile_slot_value(&tile, -5.0, &celsius_render());
        // Negative values clamp to 0 in the render step.
        assert_eq!(sv.value, -5); // tile_slot_value doesn't clamp; render does.
    }

    // ── resolve_profile ─────────────────────────────────────────────────────

    #[test]
    fn resolve_profile_by_name() {
        let profiles = ProfileSet::load_dir(std::path::Path::new("config/profiles"))
            .expect("load profiles for test");
        let p = resolve_profile(&profiles, Some("ax120"), None);
        assert!(p.is_some());
        assert_eq!(p.unwrap().name, "AX120_DIGITAL");
    }

    #[test]
    fn resolve_profile_by_pm() {
        let profiles = ProfileSet::load_dir(std::path::Path::new("config/profiles"))
            .expect("load profiles for test");
        let p = resolve_profile(&profiles, None, Some(3)); // AX120 claims PM 1..3
        assert!(p.is_some());
        assert_eq!(p.unwrap().name, "AX120_DIGITAL");
    }

    #[test]
    fn resolve_profile_force_wins_over_pm() {
        let profiles = ProfileSet::load_dir(std::path::Path::new("config/profiles"))
            .expect("load profiles for test");
        // Force pa120 even though PM 3 belongs to ax120.
        let p = resolve_profile(&profiles, Some("pa120"), Some(3));
        assert!(p.is_some());
        assert_eq!(p.unwrap().name, "PA120_DIGITAL");
    }

    #[test]
    fn resolve_profile_unknown_pm_is_none() {
        let profiles = ProfileSet::load_dir(std::path::Path::new("config/profiles"))
            .expect("load profiles for test");
        let p = resolve_profile(&profiles, None, Some(200));
        assert!(p.is_none());
    }

    #[test]
    fn resolve_profile_no_force_no_pm_is_none() {
        let profiles = ProfileSet::load_dir(std::path::Path::new("config/profiles"))
            .expect("load profiles for test");
        let p = resolve_profile(&profiles, None, None);
        assert!(p.is_none());
    }
}
