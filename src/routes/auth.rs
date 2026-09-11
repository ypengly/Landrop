//! PIN authentication.
//!
//! When `--pin` is set, every API route except `/api/auth`, `/api/status`
//! and `/api/qr` requires a valid session token in the `X-Landrop-Token`
//! header. Clients obtain a token by POSTing the correct PIN here. Tokens
//! are random, held only in memory, and reset on every server restart.
//!
//! When no PIN is configured, [`require_auth`] is a no-op — LANdrop trusts
//! the local network in that mode, as documented prominently in the UI and
//! README.

use crate::models::ApiError;
use crate::security;
use crate::server::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct AuthRequest {
    pin: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    token: String,
}

pub async fn authenticate(
    State(state): State<AppState>,
    Json(req): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<ApiError>)> {
    let Some(configured_pin) = &state.config.pin else {
        // No PIN configured: hand out a token anyway so the frontend flow
        // is uniform, but anyone can obtain one.
        return Ok(Json(AuthResponse {
            token: issue_token(&state),
        }));
    };

    if req.pin.trim() != configured_pin {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError {
                error: "invalid_pin".into(),
                message: "Incorrect PIN".into(),
            }),
        ));
    }

    Ok(Json(AuthResponse {
        token: issue_token(&state),
    }))
}

fn issue_token(state: &AppState) -> String {
    let token = security::generate_token();
    state
        .session_tokens
        .lock()
        .expect("token set lock poisoned")
        .insert(token.clone());
    token
}

/// Call this at the top of any handler that should require authentication
/// when a PIN is configured.
pub fn require_auth(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, Json<ApiError>)> {
    if state.config.pin.is_none() {
        return Ok(());
    }

    let token = headers
        .get("X-Landrop-Token")
        .and_then(|v| v.to_str().ok());

    match token {
        Some(t) if state.session_tokens.lock().expect("lock poisoned").contains(t) => Ok(()),
        _ => Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError {
                error: "unauthorized".into(),
                message: "Missing or invalid session token. Authenticate with your PIN first.".into(),
            }),
        )),
    }
}
