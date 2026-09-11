//! LANdrop core library.
//!
//! Split into a lib + thin binary (`src/main.rs`) so integration tests
//! (`tests/`) can build the real `axum::Router` in-process via
//! `tower::ServiceExt::oneshot` instead of needing a live TCP socket.

pub mod config;
pub mod discovery;
pub mod models;
pub mod routes;
pub mod security;
pub mod server;
pub mod storage;
pub mod websocket;
