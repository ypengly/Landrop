//! Integration tests that exercise the real axum `Router` in-process
//! (no live TCP socket needed) via `tower::ServiceExt::oneshot`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use landrop::config::Config;
use landrop::models::ServerStatus;
use landrop::server::{build_router, AppState};
use landrop::storage::Store;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Build a fully-wired `AppState` rooted at a fresh temp directory, with
/// history persistence disabled so tests don't touch the real filesystem
/// outside of the temp dir.
fn test_state(pin: Option<String>) -> (AppState, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let directory = tmp.path().join("received");
    let data_dir = tmp.path().join(".landrop");

    let config = Config {
        port: 0,
        bind: "127.0.0.1".into(),
        directory,
        data_dir,
        pin,
        discovery_enabled: false,
        max_file_size_bytes: 10 * 1024 * 1024,
        history_enabled: false,
        log_level: "error".into(),
    };

    let state = AppState {
        config: Arc::new(config),
        store: Arc::new(Store::new(None)),
        ws_tx: landrop::websocket::new_channel(),
        session_tokens: Arc::new(Mutex::new(HashSet::new())),
        device_id: "test-device".into(),
        device_name: "Test Device".into(),
        local_addr: "http://127.0.0.1:0".into(),
    };

    (state, tmp)
}

#[tokio::test]
async fn status_endpoint_returns_ok_without_auth() {
    let (state, _tmp) = test_state(None);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let status: ServerStatus = serde_json::from_slice(&body).unwrap();
    assert!(status.online);
    assert!(!status.pin_required);
}

#[tokio::test]
async fn history_requires_auth_when_pin_configured() {
    let (state, _tmp) = test_state(Some("1234".into()));
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_with_correct_pin_issues_working_token() {
    let (state, _tmp) = test_state(Some("1234".into()));
    let app = build_router(state);

    let auth_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"pin":"1234"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(auth_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(auth_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token = json["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 32);

    let history_response = app
        .oneshot(
            Request::builder()
                .uri("/api/history")
                .header("X-Landrop-Token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(history_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_with_wrong_pin_is_rejected() {
    let (state, _tmp) = test_state(Some("1234".into()));
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"pin":"0000"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn download_of_unknown_transfer_is_404() {
    let (state, _tmp) = test_state(None);
    let app = build_router(state);

    let id = uuid::Uuid::new_v4();
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/download/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn qr_endpoint_returns_svg() {
    let (state, _tmp) = test_state(None);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/qr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.contains("svg"));
}
