//! Runtime configuration for LANdrop.
//!
//! Configuration can come from CLI flags (see [`Args`]) or fall back to
//! sensible defaults so that `cargo run` works out of the box with zero
//! configuration.

use clap::Parser;
use std::path::PathBuf;

/// LANdrop — secure, local-first file sharing for your LAN.
#[derive(Parser, Debug, Clone)]
#[command(name = "landrop", author, version, about, long_about = None)]
pub struct Args {
    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Directory where received files are stored.
    #[arg(long, default_value = "./received")]
    pub directory: PathBuf,

    /// Optional PIN required to access the web UI / API (4-8 digits).
    #[arg(long)]
    pub pin: Option<String>,

    /// Disable LAN discovery (UDP broadcast announcements).
    #[arg(long)]
    pub no_discovery: bool,

    /// Maximum accepted file size, in megabytes.
    #[arg(long, default_value_t = 4096)]
    pub max_file_size_mb: u64,

    /// Disable persisting transfer history to disk.
    #[arg(long)]
    pub no_history: bool,

    /// Logging level: error, warn, info, debug, trace.
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Bind address. Defaults to all interfaces so LAN peers can connect.
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: String,
}

/// Fully-resolved configuration handed to the rest of the application.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub bind: String,
    pub directory: PathBuf,
    pub data_dir: PathBuf,
    pub pin: Option<String>,
    pub discovery_enabled: bool,
    pub max_file_size_bytes: u64,
    pub history_enabled: bool,
    pub log_level: String,
}

impl Config {
    pub fn from_args(args: Args) -> anyhow::Result<Self> {
        if let Some(pin) = &args.pin {
            if !(4..=8).contains(&pin.len()) || !pin.chars().all(|c| c.is_ascii_digit()) {
                anyhow::bail!("--pin must be 4-8 digits");
            }
        }

        let data_dir = args.directory.join(".landrop");

        Ok(Self {
            port: args.port,
            bind: args.bind,
            directory: args.directory,
            data_dir,
            pin: args.pin,
            discovery_enabled: !args.no_discovery,
            max_file_size_bytes: args.max_file_size_mb.saturating_mul(1024 * 1024),
            history_enabled: !args.no_history,
            log_level: args.log_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> Args {
        Args {
            port: 8080,
            directory: PathBuf::from("./received"),
            pin: None,
            no_discovery: false,
            max_file_size_mb: 100,
            no_history: false,
            log_level: "info".into(),
            bind: "0.0.0.0".into(),
        }
    }

    #[test]
    fn defaults_produce_valid_config() {
        let cfg = Config::from_args(base_args()).unwrap();
        assert_eq!(cfg.port, 8080);
        assert!(cfg.discovery_enabled);
        assert!(cfg.history_enabled);
        assert_eq!(cfg.max_file_size_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn accepts_valid_pin() {
        let mut args = base_args();
        args.pin = Some("1234".into());
        assert!(Config::from_args(args).is_ok());
    }

    #[test]
    fn rejects_non_numeric_pin() {
        let mut args = base_args();
        args.pin = Some("abcd".into());
        assert!(Config::from_args(args).is_err());
    }

    #[test]
    fn rejects_too_short_pin() {
        let mut args = base_args();
        args.pin = Some("12".into());
        assert!(Config::from_args(args).is_err());
    }

    #[test]
    fn no_discovery_flag_disables_discovery() {
        let mut args = base_args();
        args.no_discovery = true;
        let cfg = Config::from_args(args).unwrap();
        assert!(!cfg.discovery_enabled);
    }
}
