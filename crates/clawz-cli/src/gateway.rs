//! `clawz gateway status|start|stop`

use anyhow::{Context, Result};
use std::path::Path;

use crate::client::{worker_health, GatewayClient};
use crate::config;

pub async fn status() -> Result<()> {
    let cfg = config::resolve();
    println!("Gateway: {}", cfg.gateway_url);
    println!("Worker:  {}", cfg.worker_url);

    let gw = GatewayClient::new(&cfg);
    match gw.system_health().await {
        Ok(v) => println!("Gateway health: {}", serde_json::to_string_pretty(&v)?),
        Err(e) => println!("Gateway health: unavailable ({e})"),
    }

    match worker_health(&cfg.worker_url).await {
        Ok(v) => println!("Worker health: {}", serde_json::to_string_pretty(&v)?),
        Err(e) => println!("Worker health: unavailable ({e})"),
    }
    Ok(())
}

pub async fn start() -> Result<()> {
    let root = find_compose_root()
        .context("no docker-compose.yml found — run from the ClawZ repo or set CLAWZ_REPO")?;
    println!("Starting stack in {} …", root.display());
    let status = tokio::process::Command::new("docker")
        .args(["compose", "up", "-d"])
        .current_dir(&root)
        .status()
        .await
        .context("docker compose up")?;
    if !status.success() {
        anyhow::bail!("docker compose exited with {status}");
    }
    println!("Stack started. Run `clawz doctor` to verify.");
    Ok(())
}

pub async fn stop() -> Result<()> {
    let root = find_compose_root().context("no docker-compose.yml found")?;
    println!("Stopping stack in {} …", root.display());
    let status = tokio::process::Command::new("docker")
        .args(["compose", "down"])
        .current_dir(&root)
        .status()
        .await
        .context("docker compose down")?;
    if !status.success() {
        anyhow::bail!("docker compose exited with {status}");
    }
    println!("Stack stopped.");
    Ok(())
}

fn find_compose_root() -> Option<std::path::PathBuf> {
    if let Ok(repo) = std::env::var("CLAWZ_REPO") {
        let p = Path::new(&repo);
        if p.join("docker-compose.yml").exists() {
            return Some(p.to_path_buf());
        }
    }
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..6 {
        if dir.join("docker-compose.yml").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}
