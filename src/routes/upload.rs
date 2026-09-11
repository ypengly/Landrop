//! Handles incoming file uploads.
//!
//! Uploaded files are streamed straight to a temporary holding area under
//! `<directory>/.landrop/tmp` while progress is broadcast over WebSocket.
//! The transfer starts in `Pending` and is only moved into the real
//! receiving directory once the device owner accepts it from the dashboard
//! (see `routes::download::respond_to_incoming`) — this is what powers the
//! "Incoming file / Accept / Reject" flow from the spec and guarantees
//! LANdrop never silently overwrites or auto-saves a file the owner hasn't
//! approved.

use crate::models::{ApiError, Direction, TransferRecord, TransferStatus, WsEvent};
use crate::routes::auth::require_auth;
use crate::security;
use crate::server::AppState;
use crate::websocket::publish;
use axum::extract::{Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use chrono::Utc;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

#[derive(Serialize)]
pub struct UploadResponse {
    pub transfer_id: Uuid,
    pub status: &'static str,
}

/// How often (in bytes) to emit a progress event. Emitting on every chunk
/// would flood slow clients on a fast LAN; this keeps updates smooth
/// without saturating the WebSocket.
const PROGRESS_STEP_BYTES: u64 = 256 * 1024;

pub async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, (StatusCode, Json<ApiError>)> {
    require_auth(&state, &headers)?;

    let tmp_dir = state.config.data_dir.join("tmp");
    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        return Err(server_error(format!("could not prepare temp storage: {e}")));
    }

    // Find the first "file" field. Browsers may send extra fields (e.g. a
    // "sender" name); we only require the file itself.
    let mut sender_name = "Unknown device".to_string();
    // "download" (default): an incoming file from a peer, held for
    // accept/reject. "upload": the dashboard owner deliberately sharing
    // one of their own files, stored immediately so peers can fetch it.
    let mut direction = Direction::Download;

    loop {
        let field = multipart
            .next_field()
            .await
            .map_err(|e| bad_request(format!("malformed upload: {e}")))?;

        let Some(field) = field else {
            return Err(bad_request("no file field present in upload".into()));
        };

        let field_name = field.name().unwrap_or("").to_string();

        if field_name == "sender" {
            if let Ok(text) = field.text().await {
                if !text.trim().is_empty() {
                    sender_name = text;
                }
            }
            continue;
        }

        if field_name == "direction" {
            if let Ok(text) = field.text().await {
                if text.trim() == "upload" {
                    direction = Direction::Upload;
                }
            }
            continue;
        }

        if field_name != "file" {
            continue; // ignore unknown fields
        }

        let original_name = field
            .file_name()
            .ok_or_else(|| bad_request("upload is missing a filename".into()))?
            .to_string();

        return receive_field(state, field, original_name, sender_name, direction).await;
    }
}

async fn receive_field(
    state: AppState,
    mut field: axum::extract::multipart::Field<'_>,
    original_name: String,
    sender_name: String,
    direction: Direction,
) -> Result<Json<UploadResponse>, (StatusCode, Json<ApiError>)> {
    let safe_name = security::sanitize_filename(&original_name)
        .map_err(|e| bad_request(format!("invalid filename: {e}")))?;

    let id = Uuid::new_v4();
    let tmp_path = state
        .config
        .data_dir
        .join("tmp")
        .join(format!("{id}_{safe_name}"));

    let record = TransferRecord {
        id,
        filename: safe_name.clone(),
        size_bytes: 0, // unknown until the stream completes (chunked transfer)
        bytes_transferred: 0,
        sender: sender_name,
        receiver: state.device_name.clone(),
        direction,
        status: TransferStatus::InProgress,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        error: None,
    };
    state.store.insert_transfer(record.clone());
    publish(&state, WsEvent::TransferStarted(record.clone()));

    let mut file = match tokio::fs::File::create(&tmp_path).await {
        Ok(f) => f,
        Err(e) => {
            fail_transfer(&state, id, format!("could not create temp file: {e}"));
            return Err(server_error("failed to open storage for writing".into()));
        }
    };

    let mut total: u64 = 0;
    let mut since_last_event: u64 = 0;
    let max_size = state.config.max_file_size_bytes;

    loop {
        let chunk = match field.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                fail_transfer(&state, id, format!("upload stream error: {e}"));
                return Err(bad_request("upload interrupted".into()));
            }
        };

        total += chunk.len() as u64;

        if total > max_size {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            fail_transfer(&state, id, "file exceeds maximum allowed size".into());
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ApiError {
                    error: "file_too_large".into(),
                    message: format!(
                        "File exceeds the configured maximum of {} bytes",
                        max_size
                    ),
                }),
            ));
        }

        if let Err(e) = file.write_all(&chunk).await {
            fail_transfer(&state, id, format!("disk write error: {e}"));
            return Err(server_error("failed writing file to disk".into()));
        }

        since_last_event += chunk.len() as u64;
        if since_last_event >= PROGRESS_STEP_BYTES {
            since_last_event = 0;
            if let Some(updated) = state.store.update_transfer(id, |r| {
                r.bytes_transferred = total;
                r.size_bytes = total; // best-effort running total for the progress bar
            }) {
                publish(&state, WsEvent::TransferProgress(updated));
            }
        }
    }

    if let Err(e) = file.flush().await {
        fail_transfer(&state, id, format!("failed to finalize file: {e}"));
        return Err(server_error("failed to finalize upload".into()));
    }

    if direction == Direction::Upload {
        // The owner is sharing their own file: move it into the receiving
        // directory immediately and mark it complete/downloadable.
        let final_path = match unique_destination(&state.config.directory, &safe_name).await {
            Ok(p) => p,
            Err(e) => {
                fail_transfer(&state, id, format!("could not finalize file: {e}"));
                return Err(server_error("failed to store shared file".into()));
            }
        };
        if let Err(e) = move_file(&tmp_path, &final_path).await {
            fail_transfer(&state, id, format!("could not move file into place: {e}"));
            return Err(server_error("failed to store shared file".into()));
        }
        let final_record = state
            .store
            .update_transfer(id, |r| {
                r.bytes_transferred = total;
                r.size_bytes = total;
                r.status = TransferStatus::Completed;
            })
            .expect("transfer record must exist");
        publish(&state, WsEvent::TransferCompleted(final_record));
        return Ok(Json(UploadResponse {
            transfer_id: id,
            status: "completed",
        }));
    }

    let final_record = state
        .store
        .update_transfer(id, |r| {
            r.bytes_transferred = total;
            r.size_bytes = total;
            r.status = TransferStatus::Pending; // awaiting owner accept/reject
        })
        .expect("transfer record must exist");

    publish(&state, WsEvent::IncomingFile(final_record));

    Ok(Json(UploadResponse {
        transfer_id: id,
        status: "pending_approval",
    }))
}

/// Find a filesystem path under `dir` for `filename` that does not already
/// exist, appending " (1)", " (2)", etc. before the extension on
/// collision. LANdrop never silently overwrites an existing file.
pub async fn unique_destination(
    dir: &std::path::Path,
    filename: &str,
) -> Result<std::path::PathBuf, security::SecurityError> {
    let candidate = security::safe_join(dir, filename)?;
    if !candidate.exists() {
        return Ok(candidate);
    }

    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{s}"))
        .unwrap_or_default();

    for n in 1..10_000 {
        let name = format!("{stem} ({n}){ext}");
        let path = security::safe_join(dir, &name)?;
        if !path.exists() {
            return Ok(path);
        }
    }
    // Extremely unlikely fallback: use the transfer's own uniqueness.
    security::safe_join(dir, &format!("{stem}-{}{ext}", Uuid::new_v4()))
}

/// Move a file from the temp holding area into its final destination,
/// falling back to copy+delete if the rename fails (e.g. across
/// filesystems/mount points).
pub async fn move_file(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(_) => {
            tokio::fs::copy(from, to).await?;
            tokio::fs::remove_file(from).await?;
            Ok(())
        }
    }
}

fn fail_transfer(state: &AppState, id: Uuid, message: String) {
    if let Some(updated) = state.store.update_transfer(id, |r| {
        r.status = TransferStatus::Failed;
        r.error = Some(message);
    }) {
        publish(state, WsEvent::TransferFailed(updated));
    }
}

fn bad_request(message: String) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: "bad_request".into(),
            message,
        }),
    )
}

fn server_error(message: String) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: "internal_error".into(),
            message,
        }),
    )
}
