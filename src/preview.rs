//! Live web preview — a self-contained HTML page that polls the latest
//! rendered frame and draws it as colored LEDs.
//!
//! Enabled by `api.preview_enabled`. Disabled by default (zero cost).
//!
//! # Design
//!
//! * Embedded HTML (CSS + JS inlined) served via `include_str!`.
//! * Single polling endpoint `GET /preview/frame` returns the latest
//!   [`PreviewFrame`][crate::state::PreviewFrame] from shared state.
//! * No WebSocket — polling at 4Hz is sufficient for a diagnostic preview.
//! * No new dependencies — uses existing axum + serde.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::util::lock_arc;

/// Embedded self-contained HTML dashboard.
///
/// CSS and JS are inlined — zero external asset requests at runtime.
const PREVIEW_HTML: &str = include_str!("../static/preview.html");

/// Build the preview router (just two routes).
///
/// # Arguments
///
/// * `state` — shared application state.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_page))
        .route("/frame", get(get_frame))
        .with_state(state)
}

/// GET /preview — serve the self-contained HTML dashboard.
async fn serve_page() -> Html<&'static str> {
    Html(PREVIEW_HTML)
}

/// GET /preview/frame — return the latest rendered frame as JSON.
///
/// Returns 503 if no frame has been rendered yet (device not identified).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrameResponse {
    generation: u64,
    profile: String,
    leds: Vec<[u8; 3]>,
}

async fn get_frame(State(st): State<AppState>) -> Result<Json<FrameResponse>, StatusCode> {
    let s = lock_arc(&st.shared);
    match &s.preview_frame {
        Some(f) => Ok(axum::Json(FrameResponse {
            generation: f.generation,
            profile: f.profile.clone(),
            leds: f.leds.clone(),
        })),
        None => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

use axum::Json;