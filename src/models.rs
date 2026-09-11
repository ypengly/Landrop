//! Shared data types used across the server, routes, storage and websocket
//! layers. Keeping these in one module avoids circular imports between
//! `routes/*` and `storage`/`websocket`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Direction of a transfer relative to *this* LANdrop instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// A file sent from this device to a peer.
    Upload,
    /// A file received on this device from a peer.
    Download,
}

/// Lifecycle status of a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    Rejected,
}

/// A single row in the transfer history / active-transfer table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRecord {
    pub id: Uuid,
    pub filename: String,
    pub size_bytes: u64,
    pub bytes_transferred: u64,
    pub sender: String,
    pub receiver: String,
    pub direction: Direction,
    pub status: TransferStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub error: Option<String>,
}

impl TransferRecord {
    pub fn progress_percent(&self) -> f64 {
        if self.size_bytes == 0 {
            return 100.0;
        }
        (self.bytes_transferred as f64 / self.size_bytes as f64) * 100.0
    }
}

/// A device that has been seen either via LAN discovery broadcasts or by
/// connecting to the web UI / API directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub address: String,
    pub last_seen: DateTime<Utc>,
    pub connected: bool,
}

/// Outbound payload broadcast to every connected WebSocket client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WsEvent {
    TransferStarted(TransferRecord),
    TransferProgress(TransferRecord),
    TransferCompleted(TransferRecord),
    TransferFailed(TransferRecord),
    DeviceJoined(Device),
    DeviceLeft { id: String },
    IncomingFile(TransferRecord),
}

/// Server status payload returned by `GET /api/status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub online: bool,
    pub version: &'static str,
    pub address: String,
    pub port: u16,
    pub directory: String,
    pub discovery_enabled: bool,
    pub pin_required: bool,
    pub max_file_size_bytes: u64,
    pub connected_devices: usize,
}

/// Standard JSON error body returned by the API.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub message: String,
}
