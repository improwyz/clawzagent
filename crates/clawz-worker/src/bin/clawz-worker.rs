//! `clawz-worker` — agent worker with HTTP control API.
//!
//! Run with: `cargo run --bin clawz-worker`
//! Listens on `CLAWZ_CONTROL_ADDR` (default `0.0.0.0:50051`).

use std::net::SocketAddr;
use std::sync::Arc;

use clawz_worker::control_api::{self, ControlState};
use clawz_worker::service::WorkerService;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let control_addr: SocketAddr = std::env::var("CLAWZ_CONTROL_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:50051".into())
        .parse()
        .expect("CLAWZ_CONTROL_ADDR must be a valid socket address");

    if let Ok(url) = std::env::var("DATABASE_URL") {
        match sqlx::PgPool::connect(&url).await {
            Ok(pool) => {
                if let Err(e) = clawz_core::db::run_migrations(&pool).await {
                    tracing::warn!("worker database migration: {e}");
                } else {
                    tracing::info!("worker database ready");
                }
            }
            Err(e) => tracing::warn!("worker database connect failed: {e}"),
        }
    }

    let service = Arc::new(WorkerService::new().await?);
    let app = control_api::routes(ControlState { service });

    let listener = tokio::net::TcpListener::bind(control_addr).await?;
    tracing::info!("clawz-worker control API listening on {}", control_addr);

    axum::serve(listener, app).await?;
    Ok(())
}
