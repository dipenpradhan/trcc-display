//! Optional REST control surface (axum). Lets you read status and drive the
//! display by hand: set a slot's value, push a raw frame, blank it, or reload
//! config. Disabled entirely when `api.enabled = false`.
//!
//! # Endpoints
//!
//! | Method & path        | Body                                             |
//! |----------------------|--------------------------------------------------|
//! | `GET  /health`       | —                                                |
//! | `GET  /status`       | —                                                |
//! | `GET  /config`       | —                                                |
//! | `POST /reload`       | —                                                |
//! | `POST /display/value`| `{slot, value, unit?, color?[3], ttl_seconds?}`  |
//! | `POST /display/raw`  | `{colors:[[r,g,b]...], ttl_seconds?}`            |
//! | `POST /display/off`  | `{ttl_seconds?}`                                 |
//! | `POST /display/clear`| —                                                |
//!
//! # Thread safety
//!
//! All handlers access shared state via `Arc<Mutex<Shared>>`. The axum router
//! holds `AppState` which wraps the shared references.
//!
//! # Shutdown
//!
//! The server shuts down gracefully when the `shutdown` future resolves
//! (typically on Ctrl-C).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::preview;
use crate::profile::ProfileSet;
use crate::protocol::Rgb;
use crate::state::{Override, RawOverride, Shared};
use crate::util::lock_arc;

/// Shared state handed to every handler.
///
/// Wraps references to shared runtime state, config, and profile data.
/// Cloneable for use with axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    /// Shared runtime state (values, overrides, connection status).
    pub shared: Arc<Mutex<Shared>>,
    /// Shared config (can be hot-reloaded).
    pub config: Arc<Mutex<Config>>,
    /// Path to the config file.
    pub config_path: PathBuf,
    /// Loaded device profiles.
    pub profiles: Arc<ProfileSet>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("config_path", &self.config_path)
            .finish_non_exhaustive()
    }
}

/// Build the router. Exposed so integration tests can exercise handlers without
/// binding a socket.
///
/// # Arguments
///
/// * `state` — shared application state.
/// * `preview_enabled` — when `true`, nests the live preview under `/preview`.
pub fn router(state: AppState, preview_enabled: bool) -> Router {
    let mut r = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/status", get(status))
        .route("/config", get(config))
        .route("/reload", post(reload))
        .route("/display/value", post(set_value))
        .route("/display/raw", post(set_raw))
        .route("/display/off", post(display_off))
        .route("/display/clear", post(clear))
        .with_state(state.clone());
    if preview_enabled {
        r = r.nest("/preview", preview::router(state));
    }
    r
}

/// Serve the router on `bind` until `shutdown` resolves.
///
/// # Arguments
///
/// * `state` — shared application state.
/// * `bind` — listen address (e.g. `"0.0.0.0:9110"`).
/// * `shutdown` — future that resolves when the server should stop.
pub async fn serve(
    state: AppState,
    bind: &str,
    preview_enabled: bool,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| anyhow::anyhow!("binding REST API to {bind}: {e}"))?;
    tracing::info!(%bind, preview_enabled, "REST API listening");
    axum::serve(listener, router(state, preview_enabled))
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| anyhow::anyhow!("REST server error: {e}"))
}

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": msg.into() })),
    )
}

fn ok(msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "message": msg.into() })),
    )
}

/// GET /status — return connection status, values, errors, and overrides.
async fn status(State(st): State<AppState>) -> impl IntoResponse {
    let s = lock_arc(&st.shared);
    Json(json!({
        "connected": s.connected,
        "pm": s.pm,
        "sub": s.sub,
        "profile": s.profile_name,
        "source": s.source_kind,
        "frames_sent": s.frames_sent,
        "last_frame_ms_ago": s.last_frame_at.map(|t| t.elapsed().as_millis()),
        "values": s.values,
        "errors": s.errors,
        "overrides": s.overrides.keys().collect::<Vec<_>>(),
        "raw_override": s.raw_override.is_some(),
        "last_error": s.last_error,
    }))
}

/// GET /config — return the current config file as JSON.
async fn config(State(st): State<AppState>) -> impl IntoResponse {
    // Config isn't Serialize; re-read the file for the canonical view.
    match std::fs::read_to_string(&st.config_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        Some(v) => (StatusCode::OK, Json(v)),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "could not read config file" })),
        ),
    }
}

/// POST /reload — re-read the config file.
async fn reload(State(st): State<AppState>) -> impl IntoResponse {
    match Config::load(&st.config_path) {
        Ok(new) => {
            let restart_needed = {
                let cur = lock_arc(&st.config);
                cur.source.kind != new.source.kind
                    || cur.prometheus.url != new.prometheus.url
                    || cur.api.bind != new.api.bind
            };
            *lock_arc(&st.config) = new;
            ok(if restart_needed {
                "reloaded tiles/render; source/url/bind changes need a restart"
            } else {
                "reloaded"
            })
        }
        Err(e) => bad_request(format!("reload failed: {e:#}")),
    }
}

#[derive(Deserialize)]
struct ValueReq {
    slot: String,
    value: f64,
    unit: Option<String>,
    color: Option<[u8; 3]>,
    ttl_seconds: Option<u64>,
}

/// POST /display/value — set a slot's value.
async fn set_value(State(st): State<AppState>, Json(req): Json<ValueReq>) -> impl IntoResponse {
    let profile_name = lock_arc(&st.shared).profile_name.clone();
    if let Some(name) = &profile_name {
        if let Some(p) = st.profiles.by_name(name) {
            if !p.slots.contains_key(&req.slot) {
                let slots: Vec<_> = p.slots.keys().collect();
                return bad_request(format!(
                    "slot {:?} not in profile {name}; available: {slots:?}",
                    req.slot
                ));
            }
        }
    }
    let color: Rgb = req
        .color
        .map_or(Rgb(255, 255, 255), |c| Rgb(c[0], c[1], c[2]));
    let expires_at = req
        .ttl_seconds
        .map(|s| Instant::now() + Duration::from_secs(s));
    lock_arc(&st.shared).overrides.insert(
        req.slot.clone(),
        Override {
            value: req.value.round() as i64,
            unit: req.unit,
            color,
            expires_at,
        },
    );
    ok(format!(
        "slot {:?} set to {}",
        req.slot,
        req.value.round() as i64
    ))
}

#[derive(Deserialize)]
struct RawReq {
    colors: Vec<[u8; 3]>,
    ttl_seconds: Option<u64>,
}

/// POST /display/raw — push a full frame.
async fn set_raw(State(st): State<AppState>, Json(req): Json<RawReq>) -> impl IntoResponse {
    if let Some(name) = lock_arc(&st.shared).profile_name.clone() {
        if let Some(p) = st.profiles.by_name(&name) {
            if req.colors.len() != p.mask_size {
                return bad_request(format!(
                    "profile {name} needs exactly {} colors, got {}",
                    p.mask_size,
                    req.colors.len()
                ));
            }
        }
    }
    let colors: Vec<Rgb> = req.colors.iter().map(|c| Rgb(c[0], c[1], c[2])).collect();
    let expires_at = req
        .ttl_seconds
        .map(|s| Instant::now() + Duration::from_secs(s));
    lock_arc(&st.shared).raw_override = Some(RawOverride { colors, expires_at });
    ok("raw frame set")
}

#[derive(Deserialize)]
struct OffReq {
    ttl_seconds: Option<u64>,
}

/// POST /display/off — blank the display.
async fn display_off(State(st): State<AppState>, Json(req): Json<OffReq>) -> impl IntoResponse {
    let size = lock_arc(&st.shared)
        .profile_name
        .clone()
        .and_then(|n| st.profiles.by_name(&n).map(|p| p.mask_size));
    let Some(size) = size else {
        return bad_request("no active profile yet (device not identified)");
    };
    let expires_at = req
        .ttl_seconds
        .map(|s| Instant::now() + Duration::from_secs(s));
    lock_arc(&st.shared).raw_override = Some(RawOverride {
        colors: vec![Rgb(0, 0, 0); size],
        expires_at,
    });
    ok("display blanked")
}

/// POST /display/clear — remove all overrides.
async fn clear(State(st): State<AppState>) -> impl IntoResponse {
    let mut s = lock_arc(&st.shared);
    s.overrides.clear();
    s.raw_override = None;
    ok("cleared all overrides")
}

/// A serializable snapshot of status (used by the `status` CLI subcommand).
#[derive(Debug, Serialize)]
pub struct StatusView {
    /// `true` while the USB device is connected.
    pub connected: bool,
    /// Active profile name.
    pub profile: Option<String>,
    /// Active metric source.
    pub source: String,
    /// Cumulative frames sent.
    pub frames_sent: u64,
}
