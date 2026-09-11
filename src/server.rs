//! Application state and router assembly.

use crate::config::Config;
use crate::models::WsEvent;
use crate::routes;
use crate::storage::Store;
use crate::websocket;
use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::Router;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Shared, cheaply-cloneable application state. Every route handler takes
/// `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Arc<Store>,
    pub ws_tx: broadcast::Sender<WsEvent>,
    /// Randomly generated at startup; required for authenticated API calls
    /// once a client has verified the PIN (or immediately, if no PIN is
    /// configured). See `routes::auth`.
    pub session_tokens: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    pub device_id: String,
    pub device_name: String,
    pub local_addr: String,
}

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::any())
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let max_body = state.config.max_file_size_bytes as usize;

    let api = Router::new()
        .route("/status", get(routes::status::status))
        .route("/qr", get(routes::status::qr_code))
        .route("/devices", get(routes::devices::list_devices))
        .route("/upload", post(routes::upload::upload_file))
        .route("/download/:id", get(routes::download::download_file))
        .route(
            "/transfers/:id",
            delete(routes::download::delete_transfer)
                .post(routes::download::respond_to_incoming),
        )
        .route("/history", get(routes::download::history))
        .route("/auth", post(routes::auth::authenticate))
        .layer(DefaultBodyLimit::max(max_body.max(1024 * 1024)));

    Router::new()
        .route("/", get(index))
        .route("/ws", get(websocket::ws_handler))
        .nest("/api", api)
        .nest_service("/static", tower_http::services::ServeDir::new("web"))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    match tokio::fs::read_to_string("web/index.html").await {
        Ok(html) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))],
            html,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "web/index.html not found").into_response(),
    }
}
