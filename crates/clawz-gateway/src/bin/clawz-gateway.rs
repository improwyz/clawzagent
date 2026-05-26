//! `clawz-gateway` — API gateway entry point for ClawZ agent platform.
//!
//! Run with: `cargo run --bin clawz-gateway`
//! Listens on `CLAWZ_LISTEN_ADDR` (default `0.0.0.0:3000`).

use clawz_gateway::{server::GatewayServer, AppState};
use std::net::SocketAddr;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let listen_addr: SocketAddr = std::env::var("CLAWZ_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3000".into())
        .parse()
        .expect("CLAWZ_LISTEN_ADDR must be a valid socket address");

    let jwt_secret = std::env::var("CLAWZ_JWT_SECRET")
        .unwrap_or_else(|_| "dev-secret-change-in-production".to_string());

    // Optionally inject identity store if CLAWZ_IDENTITY_STORE path is set
    let identity_store = std::env::var("CLAWZ_IDENTITY_STORE")
        .ok()
        .map(|_path| {
            std::sync::Arc::new(
                clawz_worker::runtime::identity::AgentIdentityStore::new_in_memory(),
            )
        });

    let state = AppState::with_identity_store(jwt_secret, identity_store);
    let server = GatewayServer::new(state);

    tracing::info!("clawz-gateway starting on {}", listen_addr);
    server.serve(listen_addr).await
}
