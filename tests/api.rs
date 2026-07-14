//! REST API handler tests — exercised in-process via `tower::oneshot`, no socket.
//!
//! These tests verify that all API endpoints work correctly with real profile
//! data, proper error handling, and edge cases.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use trcc_display::api::{AppState, router};
use trcc_display::config::Config;
use trcc_display::profile::ProfileSet;
use trcc_display::state::Shared;

const MINIMAL_CONFIG: &str = r#"{
    "usb": { "vendor_id": 1046, "product_id": 32769 },
    "profile": { "dir": "config/profiles" },
    "render": {},
    "tiles": []
}"#;

fn state() -> (AppState, tempfile::NamedTempFile) {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(MINIMAL_CONFIG.as_bytes()).unwrap();
    let cfg = Config::load(file.path()).unwrap();
    let profiles = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();

    let mut shared = Shared::new();
    shared.connected = true;
    shared.profile_name = Some("PA120_DIGITAL".into()); // 84 LEDs, has gpu_temp

    let st = AppState {
        shared: Arc::new(Mutex::new(shared)),
        config: Arc::new(Mutex::new(cfg)),
        config_path: file.path().to_path_buf(),
        profiles: Arc::new(profiles),
    };
    (st, file)
}

async fn send(st: AppState, req: Request<Body>) -> (StatusCode, String) {
    let resp = router(st, false).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn post(uri: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_string()))
        .unwrap()
}

// ── Health ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health() {
    let (st, _f) = state();
    let (status, body) = send(
        st,
        Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

// ── Status ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn status_reports_connection() {
    let (st, _f) = state();
    let (status, body) = send(
        st,
        Request::builder()
            .uri("/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"connected\":true"), "{body}");
    assert!(body.contains("PA120_DIGITAL"), "{body}");
}

#[tokio::test]
async fn status_reports_values() {
    let (st, _f) = state();
    let mut shared = st.shared.lock().unwrap();
    shared.values.insert("cpu".into(), 57.0);
    shared.errors.insert("gpu".into(), "timeout".into());
    drop(shared);

    let (status, body) = send(
        st,
        Request::builder()
            .uri("/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"cpu\":57"), "{body}");
    assert!(body.contains("timeout"), "{body}");
}

// ── Config ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn config_returns_json() {
    let (st, _f) = state();
    let (status, body) = send(
        st,
        Request::builder()
            .uri("/config")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Should be valid JSON.
    let _parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
}

// ── Display Value ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_value_unknown_slot_is_400() {
    let (st, _f) = state();
    let (status, body) = send(st, post("/display/value", r#"{"slot":"nope","value":50}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("not in profile"), "{body}");
}

#[tokio::test]
async fn set_value_known_slot_is_200() {
    let (st, _f) = state();
    let (status, _body) = send(
        st,
        post(
            "/display/value",
            r#"{"slot":"gpu_temp","value":63,"unit":"celsius"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn set_value_with_color() {
    let (st, _f) = state();
    let (status, body) = send(
        st,
        post(
            "/display/value",
            r#"{"slot":"gpu_temp","value":72,"color":[255,60,60]}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("set to 72"), "{body}");
}

#[tokio::test]
async fn set_value_with_ttl() {
    let (st, _f) = state();
    let (status, _body) = send(
        st,
        post(
            "/display/value",
            r#"{"slot":"gpu_temp","value":72,"ttl_seconds":30}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ── Display Raw ────────────────────────────────────────────────────────────

#[tokio::test]
async fn raw_frame_wrong_size_is_400() {
    let (st, _f) = state();
    // PA120 needs 84 colors; send 2.
    let (status, body) = send(st, post("/display/raw", r#"{"colors":[[1,2,3],[4,5,6]]}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("84 colors"), "{body}");
}

#[tokio::test]
async fn raw_frame_correct_size_is_200() {
    let (st, _f) = state();
    // PA120 needs exactly 84 colors.
    let colors: String = (0..84)
        .map(|i| format!("[{},0,0]", i))
        .collect::<Vec<_>>()
        .join(",");
    let (status, _body) = send(
        st,
        post("/display/raw", &format!("{{\"colors\":[{}]}}", colors)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ── Display Off ────────────────────────────────────────────────────────────

#[tokio::test]
async fn display_off_is_200() {
    let (st, _f) = state();
    let (status, body) = send(st, post("/display/off", "{}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("blanked"), "{body}");
}

// ── Display Clear ──────────────────────────────────────────────────────────

#[tokio::test]
async fn clear_ok() {
    let (st, _f) = state();
    let (status, _) = send(st, post("/display/clear", "")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn clear_removes_overrides() {
    let (st, _f) = state();
    // Set an override.
    {
        let mut shared = st.shared.lock().unwrap();
        shared.overrides.insert(
            "gpu_temp".into(),
            trcc_display::state::Override {
                value: 99,
                unit: Some("celsius".into()),
                color: trcc_display::protocol::Rgb(255, 0, 0),
                expires_at: None,
            },
        );
        assert_eq!(shared.overrides.len(), 1);
    }

    // Clear it.
    let (status, _body) = send(st.clone(), post("/display/clear", "")).await;
    assert_eq!(status, StatusCode::OK);

    // Verify it's gone.
    let shared = st.shared.lock().unwrap();
    assert!(shared.overrides.is_empty());
}

// ── Reload ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reload_ok() {
    let (st, _f) = state();
    let (status, body) = send(st, post("/reload", "")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("reloaded"), "{body}");
}

// ── 404 for unknown routes ────────────────────────────────────────────────

#[tokio::test]
async fn unknown_route_returns_404() {
    let (st, _f) = state();
    let (status, _) = send(
        st,
        Request::builder()
            .uri("/nonexistent")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Preview ──────────────────────────────────────────────────────────────

fn state_with_preview() -> (AppState, tempfile::NamedTempFile) {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(MINIMAL_CONFIG.as_bytes()).unwrap();
    let cfg = Config::load(file.path()).unwrap();
    let profiles = ProfileSet::load_dir(Path::new("config/profiles")).unwrap();

    let mut shared = Shared::new();
    shared.connected = true;
    shared.profile_name = Some("PA120_DIGITAL".into());

    let st = AppState {
        shared: Arc::new(Mutex::new(shared)),
        config: Arc::new(Mutex::new(cfg)),
        config_path: file.path().to_path_buf(),
        profiles: Arc::new(profiles),
    };
    (st, file)
}

async fn send_preview(st: AppState, req: Request<Body>) -> (StatusCode, String) {
    let resp = router(st, true).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn preview_serves_html() {
    let (st, _f) = state_with_preview();
    let (status, body) = send_preview(
        st,
        Request::builder()
            .uri("/preview")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!DOCTYPE html>"), "{body}");
}

#[tokio::test]
async fn preview_frame_requires_frame() {
    let (st, _f) = state_with_preview();
    let (status, _) = send_preview(
        st,
        Request::builder()
            .uri("/preview/frame")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn preview_frame_returns_json() {
    let (st, _f) = state_with_preview();
    // Set a preview frame in shared state
    {
        let mut shared = st.shared.lock().unwrap();
        shared.preview_frame = Some(trcc_display::state::PreviewFrame {
            generation: 42,
            profile: "AX120_DIGITAL".into(),
            leds: vec![[255, 0, 0]; 30],
        });
    }

    let (status, body) = send_preview(
        st,
        Request::builder()
            .uri("/preview/frame")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frame: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(frame["generation"], 42);
    assert_eq!(frame["profile"], "AX120_DIGITAL");
    assert_eq!(frame["leds"].as_array().unwrap().len(), 30);
}

#[tokio::test]
async fn preview_frame_updates_on_change() {
    let (st, _f) = state_with_preview();
    // Set initial frame
    {
        let mut shared = st.shared.lock().unwrap();
        shared.preview_frame = Some(trcc_display::state::PreviewFrame {
            generation: 1,
            profile: "AX120_DIGITAL".into(),
            leds: vec![[255, 0, 0]; 30],
        });
    }

    // Fetch and verify
    let (status, body) = send_preview(
        st.clone(),
        Request::builder()
            .uri("/preview/frame")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frame: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(frame["generation"], 1);

    // Update frame
    {
        let mut shared = st.shared.lock().unwrap();
        shared.preview_frame = Some(trcc_display::state::PreviewFrame {
            generation: 2,
            profile: "AX120_DIGITAL".into(),
            leds: vec![[0, 255, 0]; 30],
        });
    }

    // Fetch and verify update
    let (status, body) = send_preview(
        st.clone(),
        Request::builder()
            .uri("/preview/frame")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frame: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(frame["generation"], 2);
}

#[tokio::test]
async fn preview_not_available_when_disabled() {
    // Router with preview disabled
    let (st, _f) = state();
    let (status, _) = send(
        st,
        Request::builder()
            .uri("/preview")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
