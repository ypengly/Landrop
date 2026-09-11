//! Real-time updates over WebSocket.
//!
//! Every connected browser tab subscribes to a single [`tokio::sync::broadcast`]
//! channel of [`WsEvent`]s. Route handlers (upload/download/devices) publish
//! events into the channel; this module only fans them out to clients as
//! JSON text frames.

use crate::models::WsEvent;
use crate::server::AppState;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tracing::{debug, warn};

/// Capacity of the broadcast channel. Slow consumers that fall this far
/// behind simply miss the oldest events (they'll still see current state
/// on their next `/api/*` poll) rather than backing up the whole server.
pub const CHANNEL_CAPACITY: usize = 256;

pub fn new_channel() -> broadcast::Sender<WsEvent> {
    let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
    tx
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.ws_tx.subscribe();

    // Forward broadcast events to this client.
    let mut send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let payload = match serde_json::to_string(&event) {
                Ok(json) => json,
                Err(e) => {
                    warn!("failed to serialize ws event: {}", e);
                    continue;
                }
            };
            if sender.send(Message::Text(payload)).await.is_err() {
                break; // client disconnected
            }
        }
    });

    // Drain incoming messages. LANdrop's clients don't need to send
    // anything over the socket today, but we still need to read the
    // stream to detect disconnects and respond to pings/pongs correctly.
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Close(_) = msg {
                break;
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    debug!("websocket client disconnected");
}

/// Publish an event to all connected clients. Errors (no subscribers) are
/// expected and ignored — it just means nobody has the dashboard open.
pub fn publish(state: &AppState, event: WsEvent) {
    let _ = state.ws_tx.send(event);
}
