//! Local network discovery.
//!
//! LANdrop instances periodically broadcast a small JSON "announce" packet
//! over UDP. Any other instance listening on the same port picks it up and
//! adds/refreshes the sender in its device list. This is best-effort: some
//! networks (guest Wi-Fi with client isolation, restrictive firewalls,
//! certain VPNs) block broadcast traffic entirely, which is why manual
//! IP/QR connection always works as a fallback regardless of discovery.

use crate::models::{Device, WsEvent};
use crate::server::AppState;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::interval;
use tracing::{debug, info, warn};

pub const DISCOVERY_PORT: u16 = 47821;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
const STALE_AFTER: chrono::Duration = chrono::Duration::seconds(20);

#[derive(Debug, Serialize, Deserialize)]
struct Announce {
    magic: String,
    id: String,
    name: String,
    port: u16,
}

const MAGIC: &str = "landrop-announce-v1";

/// Spawn the broadcast (send) and listen (receive) loops. Returns
/// immediately; the loops run for the lifetime of the process.
pub async fn spawn(state: AppState, device_id: String, device_name: String, http_port: u16) {
    let socket = match bind_broadcast_socket().await {
        Ok(s) => s,
        Err(e) => {
            warn!("discovery disabled: could not bind UDP socket: {}", e);
            return;
        }
    };

    info!("LAN discovery enabled on UDP port {}", DISCOVERY_PORT);

    let send_socket = socket.clone();
    tokio::spawn(async move {
        announce_loop(send_socket, device_id, device_name, http_port).await;
    });

    tokio::spawn(async move {
        listen_loop(socket, state).await;
    });
}

async fn bind_broadcast_socket() -> anyhow::Result<std::sync::Arc<UdpSocket>> {
    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)).await?;
    socket.set_broadcast(true)?;
    Ok(std::sync::Arc::new(socket))
}

async fn announce_loop(
    socket: std::sync::Arc<UdpSocket>,
    id: String,
    name: String,
    http_port: u16,
) {
    let announce = Announce {
        magic: MAGIC.to_owned(),
        id,
        name,
        port: http_port,
    };
    let Ok(payload) = serde_json::to_vec(&announce) else {
        warn!("failed to serialize discovery announcement");
        return;
    };

    let mut ticker = interval(ANNOUNCE_INTERVAL);
    let broadcast_addr: SocketAddr = ([255, 255, 255, 255], DISCOVERY_PORT).into();

    loop {
        ticker.tick().await;
        if let Err(e) = socket.send_to(&payload, broadcast_addr).await {
            debug!("discovery broadcast send failed (non-fatal): {}", e);
        }
    }
}

async fn listen_loop(socket: std::sync::Arc<UdpSocket>, state: AppState) {
    let mut buf = [0u8; 1024];
    loop {
        let (len, src) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                debug!("discovery receive error (non-fatal): {}", e);
                continue;
            }
        };

        let Ok(announce) = serde_json::from_slice::<Announce>(&buf[..len]) else {
            continue; // ignore malformed / foreign broadcast traffic
        };

        if announce.magic != MAGIC || announce.id == state.device_id {
            continue; // ignore non-LANdrop packets and our own echo
        }

        let address = format!("http://{}:{}", src.ip(), announce.port);
        let device = Device {
            id: announce.id.clone(),
            name: announce.name,
            address,
            last_seen: chrono::Utc::now(),
            connected: true,
        };

        let is_new = state.store.list_devices().iter().all(|d| d.id != device.id);
        state.store.upsert_device(device.clone());

        if is_new {
            info!("discovered LAN device: {} ({})", device.name, device.address);
            crate::websocket::publish(&state, WsEvent::DeviceJoined(device));
        }
    }
}

/// Periodically sweep devices that haven't announced recently and mark
/// them disconnected so the dashboard doesn't show stale "Connected"
/// devices forever after they leave the network.
pub async fn spawn_staleness_sweeper(state: AppState) {
    let mut ticker = interval(Duration::from_secs(10));
    loop {
        ticker.tick().await;
        let now = chrono::Utc::now();
        for device in state.store.list_devices() {
            if device.connected && now.signed_duration_since(device.last_seen) > STALE_AFTER {
                state.store.mark_device_disconnected(&device.id);
                crate::websocket::publish(
                    &state,
                    WsEvent::DeviceLeft { id: device.id.clone() },
                );
            }
        }
    }
}
