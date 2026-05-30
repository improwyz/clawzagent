//! Hourly memory rollups for standalone SQLite (lightweight summary rows).

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use clawz_core::deployment::DeploymentMode;
use clawz_core::error::Result;

/// Spawn a background task that writes hourly rollup markers into local memory.
pub fn spawn_hourly_rollup_task() {
    if DeploymentMode::from_env() != DeploymentMode::Standalone {
        return;
    }
    if std::env::var("CLAWZ_MEMORY_ROLLUP")
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
    {
        return;
    }

    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(e) = write_rollup_marker().await {
                log::warn!("[memory_rollup] {e}");
            }
        }
    });
}

async fn write_rollup_marker() -> Result<()> {
    let agent_id = std::env::var("CLAWZ_DEFAULT_AGENT_ID").unwrap_or_else(|_| "default".into());
    if let Ok(tree) = crate::memory::MemoryTree::open_default().await {
        let _ = tree
            .append_rollup(
                &agent_id,
                format!("hourly rollup at {}", chrono::Utc::now().to_rfc3339()),
            )
            .await;
    }
    let path = rollup_log_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            clawz_core::error::ClawzError::Internal(format!("rollup dir: {e}"))
        })?;
    }
    let line = format!("{}\n", Utc::now().to_rfc3339());
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| clawz_core::error::ClawzError::Internal(format!("rollup open: {e}")))?
        .write_all(line.as_bytes())
        .await
        .map_err(|e| clawz_core::error::ClawzError::Internal(format!("rollup write: {e}")))?;
    Ok(())
}

fn rollup_log_path() -> PathBuf {
    let home = std::env::var("CLAWZ_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".clawz")
    });
    home.join("memory_rollups.log")
}

use tokio::io::AsyncWriteExt;
