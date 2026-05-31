//! Periodic SaaS connector sync → worker memory ingest (~20 min default).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clawz_services::dto::{MemoryIngestChunk, MemoryIngestRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::connectors::{ConnectorRegistry, Filters, github::GitHubConnector};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectorSyncJob {
    platform: String,
    object: String,
    agent_id: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConnectorSyncConfig {
    #[serde(default)]
    jobs: Vec<ConnectorSyncJob>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncState {
    #[serde(default)]
    last_sync_at: Option<DateTime<Utc>>,
}

fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("CLAWZ_CONNECTOR_SYNC_FILE") {
        return PathBuf::from(p);
    }
    let home = std::env::var("CLAWZ_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".clawz")
        });
    home.join("connector-sync.json")
}

fn state_path() -> PathBuf {
    config_path().with_extension("state.json")
}

fn load_config() -> ConnectorSyncConfig {
    let path = config_path();
    if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        default_config()
    }
}

fn default_config() -> ConnectorSyncConfig {
    let mut jobs = Vec::new();
    if std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("CLAWZ_CONNECTOR_GITHUB_TOKEN"))
        .is_ok()
    {
        jobs.push(ConnectorSyncJob {
            platform: "github".into(),
            object: "issues".into(),
            agent_id: std::env::var("CLAWZ_CONNECTOR_AGENT_ID")
                .unwrap_or_else(|_| "default".into()),
            enabled: true,
        });
    }
    ConnectorSyncConfig { jobs }
}

fn load_state() -> SyncState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(state: &SyncState) -> anyhow::Result<()> {
    if let Some(parent) = state_path().parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(state_path(), serde_json::to_string_pretty(state)?)?;
    Ok(())
}

fn build_registry() -> ConnectorRegistry {
    let mut registry = ConnectorRegistry::new();
    if let Ok(token) =
        std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("CLAWZ_CONNECTOR_GITHUB_TOKEN"))
    {
        if !token.is_empty() {
            registry.register(Arc::new(GitHubConnector::with_api_key(token)));
        }
    }
    registry
}

fn object_to_text(obj: &Value) -> String {
    if let Some(title) = obj.get("title").and_then(|v| v.as_str()) {
        let body = obj.get("body").and_then(|v| v.as_str()).unwrap_or("");
        return format!("{title}\n{body}");
    }
    obj.to_string()
}

/// Start connector sync loop unless `CLAWZ_CONNECTOR_SYNC=0`.
pub fn spawn(state: Arc<AppState>) {
    if std::env::var("CLAWZ_CONNECTOR_SYNC").ok().as_deref() == Some("0") {
        tracing::info!("connector sync disabled (CLAWZ_CONNECTOR_SYNC=0)");
        return;
    }

    let interval_secs = std::env::var("CLAWZ_CONNECTOR_SYNC_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1200);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        tracing::info!("connector scheduler started (every {interval_secs}s)");
        loop {
            interval.tick().await;
            if let Err(e) = sync_tick(&state).await {
                tracing::warn!("connector sync tick: {e}");
            }
        }
    });
}

async fn sync_tick(state: &AppState) -> anyhow::Result<()> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("platform not configured"))?;

    let config = load_config();
    if config.jobs.is_empty() {
        return Ok(());
    }

    let registry = build_registry();
    let mut sync_state = load_state();
    let since = sync_state.last_sync_at;

    for job in config.jobs.into_iter().filter(|j| j.enabled) {
        let Some(connector) = registry.get(&job.platform) else {
            tracing::debug!("connector {} not registered, skipping", job.platform);
            continue;
        };
        let mut filters = Filters::default();
        filters.limit = Some(20);
        if let Some(since) = since {
            filters.updated_after = Some(since);
        }
        let objects = connector
            .list_objects(&job.object, &filters)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let chunks: Vec<MemoryIngestChunk> = objects
            .iter()
            .enumerate()
            .map(|(i, obj)| {
                let id = obj
                    .get("id")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| i.to_string());
                MemoryIngestChunk {
                    key: format!("{}:{}:{}", job.platform, job.object, id),
                    text: object_to_text(obj),
                    source: Some(job.platform.clone()),
                }
            })
            .filter(|c| !c.text.trim().is_empty())
            .collect();

        if chunks.is_empty() {
            continue;
        }

        let stored = platform
            .execution
            .ingest_memory(MemoryIngestRequest {
                agent_id: job.agent_id.clone(),
                chunks,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .stored;

        tracing::info!(
            platform = %job.platform,
            object = %job.object,
            stored,
            "connector sync ingested chunks"
        );
    }

    sync_state.last_sync_at = Some(Utc::now());
    save_state(&sync_state)?;
    Ok(())
}
