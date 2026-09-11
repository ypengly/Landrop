use crate::models::ServerStatus;
use crate::server::AppState;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use qrcode::QrCode;

pub async fn status(State(state): State<AppState>) -> Json<ServerStatus> {
    Json(ServerStatus {
        online: true,
        version: env!("CARGO_PKG_VERSION"),
        address: state.local_addr.clone(),
        port: state.config.port,
        directory: state.config.directory.display().to_string(),
        discovery_enabled: state.config.discovery_enabled,
        pin_required: state.config.pin.is_some(),
        max_file_size_bytes: state.config.max_file_size_bytes,
        connected_devices: state.store.connected_device_count(),
    })
}

/// Renders a QR code encoding the local connection URL as an inline SVG.
/// Generated on the fly (cheap) rather than cached, since it depends only
/// on `local_addr`, which is fixed for the process lifetime.
pub async fn qr_code(State(state): State<AppState>) -> impl IntoResponse {
    let code = match QrCode::new(state.local_addr.as_bytes()) {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "failed to build QR code").into_response(),
    };

    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#111111"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"))],
        svg,
    )
        .into_response()
}
