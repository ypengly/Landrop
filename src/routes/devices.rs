use crate::models::{ApiError, Device};
use crate::routes::auth::require_auth;
use crate::server::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;

pub async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Device>>, (StatusCode, Json<ApiError>)> {
    require_auth(&state, &headers)?;
    Ok(Json(state.store.list_devices()))
}
