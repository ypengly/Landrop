use crate::models::{ApiError, TransferRecord, TransferStatus, WsEvent};
use crate::routes::auth::require_auth;
use crate::routes::upload::{move_file, unique_destination};
use crate::security;
use crate::server::AppState;
use crate::websocket::publish;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

pub async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TransferRecord>>, (StatusCode, Json<ApiError>)> {
    require_auth(&state, &headers)?;
    Ok(Json(state.store.list_history()))
}

pub async fn download_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, Json<ApiError>)> {
    require_auth(&state, &headers)?;

    let record = state
        .store
        .get_transfer(id)
        .ok_or_else(|| not_found("transfer not found"))?;

    if record.status != TransferStatus::Completed {
        return Err((
            StatusCode::CONFLICT,
            Json(ApiError {
                error: "not_ready".into(),
                message: "This file is not available for download yet".into(),
            }),
        ));
    }

    let path = security::safe_join(&state.config.directory, &record.filename)
        .map_err(|_| not_found("file no longer exists"))?;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| not_found("file no longer exists on disk"))?;

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let disposition = format!("attachment; filename=\"{}\"", record.filename.replace('"', ""));
    let response_headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
        (
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition).unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        ),
    ];

    Ok((StatusCode::OK, response_headers, body))
}

pub async fn delete_transfer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    require_auth(&state, &headers)?;

    if state.store.delete_transfer(id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found("transfer not found"))
    }
}

#[derive(Deserialize)]
pub struct RespondRequest {
    pub action: RespondAction,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RespondAction {
    Accept,
    Reject,
}

/// Accept or reject a `Pending` incoming file (see `routes::upload`).
pub async fn respond_to_incoming(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<RespondRequest>,
) -> Result<Json<TransferRecord>, (StatusCode, Json<ApiError>)> {
    require_auth(&state, &headers)?;

    let record = state
        .store
        .get_transfer(id)
        .ok_or_else(|| not_found("transfer not found"))?;

    if record.status != TransferStatus::Pending {
        return Err((
            StatusCode::CONFLICT,
            Json(ApiError {
                error: "not_pending".into(),
                message: "This transfer has already been resolved".into(),
            }),
        ));
    }

    let tmp_path = state
        .config
        .data_dir
        .join("tmp")
        .join(format!("{id}_{}", record.filename));

    if req.action == RespondAction::Reject {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        let updated = state
            .store
            .update_transfer(id, |r| {
                r.status = TransferStatus::Rejected;
            })
            .expect("record exists");
        publish(&state, WsEvent::TransferFailed(updated.clone()));
        return Ok(Json(updated));
    }

    let final_path = unique_destination(&state.config.directory, &record.filename)
        .await
        .map_err(|_| server_error("could not finalize destination path"))?;

    move_file(&tmp_path, &final_path)
        .await
        .map_err(|_| server_error("could not move accepted file into place"))?;

    let updated = state
        .store
        .update_transfer(id, |r| {
            r.status = TransferStatus::Completed;
            r.filename = final_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&r.filename)
                .to_string();
        })
        .expect("record exists");

    publish(&state, WsEvent::TransferCompleted(updated.clone()));
    Ok(Json(updated))
}

fn not_found(message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError {
            error: "not_found".into(),
            message: message.into(),
        }),
    )
}

fn server_error(message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: "internal_error".into(),
            message: message.into(),
        }),
    )
}
