use clap::Parser;
use landrop::config::{Args, Config};
use landrop::server::AppState;
use landrop::{discovery, server, storage::Store, websocket};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = Config::from_args(args)?;

    init_tracing(&config.log_level);

    // Make sure the receiving directory exists before we do anything else.
    tokio::fs::create_dir_all(&config.directory).await.map_err(|e| {
        anyhow::anyhow!(
            "could not create receiving directory {:?}: {e}",
            config.directory
        )
    })?;

    let local_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let local_addr = format!("http://{local_ip}:{}", config.port);

    let device_id = uuid::Uuid::new_v4().to_string();
    let device_name = hostname_or_default();

    let store_dir = if config.history_enabled {
        Some(config.data_dir.clone())
    } else {
        None
    };

    let state = AppState {
        config: Arc::new(config.clone()),
        store: Arc::new(Store::new(store_dir)),
        ws_tx: websocket::new_channel(),
        session_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        device_id: device_id.clone(),
        device_name: device_name.clone(),
        local_addr: local_addr.clone(),
    };

    print_banner(&config, &local_addr, &device_name);

    if config.discovery_enabled {
        discovery::spawn(state.clone(), device_id, device_name, config.port).await;
        tokio::spawn(discovery::spawn_staleness_sweeper(state.clone()));
    }

    let app = server::build_router(state);

    let listener = tokio::net::TcpListener::bind((config.bind.as_str(), config.port))
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind to port {}: {e}", config.port))?;

    tracing::info!("LANdrop listening on {}", local_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(format!("landrop={level},tower_http=warn"))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn hostname_or_default() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "LANdrop Device".to_string())
}

fn print_banner(config: &Config, local_addr: &str, device_name: &str) {
    println!("╭──────────────────────────────────────────╮");
    println!("│              🦀 LANdrop                   │");
    println!("╰──────────────────────────────────────────╯");
    println!();
    println!("  Device       {device_name}");
    println!("  Status       ● Running");
    println!("  Address      {local_addr}");
    println!("  Directory    {}", config.directory.display());
    println!(
        "  Discovery    {}",
        if config.discovery_enabled { "Enabled" } else { "Disabled" }
    );
    println!(
        "  PIN          {}",
        if config.pin.is_some() { "Enabled" } else { "Disabled" }
    );
    println!("  Max file     {} MB", config.max_file_size_bytes / (1024 * 1024));
    println!();
    println!("  Scan the QR code in the dashboard to connect from another device.");
    println!();
    println!("  Waiting for devices...");
    println!();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutting down gracefully...");
}
