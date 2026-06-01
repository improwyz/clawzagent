//! `clawz-agent` — per-tenant agent runtime container entrypoint.
//!
//! Spawned by the worker Bollard scheduler with `CLAWZ_AGENT_ID`, `CLAWZ_TENANT_ID`,
//! and platform URLs in the environment.

use std::net::SocketAddr;
use std::time::Duration;

use axum::{Json, Router, routing::get};
use serde_json::json;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let agent_id = std::env::var("CLAWZ_AGENT_ID").unwrap_or_else(|_| "unknown".into());
    let tenant_id = std::env::var("CLAWZ_TENANT_ID").unwrap_or_else(|_| "default".into());
    let gateway_url = std::env::var("CLAWZ_GATEWAY_URL")
        .or_else(|_| std::env::var("CLAWZ_PUBLIC_URL"))
        .unwrap_or_else(|_| "http://gateway:3000".into());
    let worker_url = std::env::var("WORKER_URL").unwrap_or_else(|_| "http://worker:50051".into());

    tracing::info!(
        agent_id = %agent_id,
        tenant_id = %tenant_id,
        gateway = %gateway_url,
        worker = %worker_url,
        "clawz-agent starting"
    );

    let health_addr: SocketAddr = std::env::var("CLAWZ_AGENT_HEALTH_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8081".into())
        .parse()
        .expect("CLAWZ_AGENT_HEALTH_ADDR must be a valid socket address");

    let app = Router::new().route(
        "/health",
        get(|| async { Json(json!({ "status": "ok", "service": "clawz-agent" })) }),
    );

    let listener = tokio::net::TcpListener::bind(health_addr).await?;
    tracing::info!("clawz-agent health listening on {}", health_addr);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("clawz-agent health server failed: {e}");
        }
    });

    // Heartbeat loop — keeps the container alive and logs platform reachability.
    loop {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            if let Ok(resp) = client.get(format!("{worker_url}/health")).send().await {
                tracing::debug!(status = %resp.status(), "worker health check");
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
