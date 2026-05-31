//! Opt-in background reflection on newly ingested memory chunks.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clawz_core::error::Result;
use clawz_services::dto::RunTurnRequest;
use serde::{Deserialize, Serialize};

use crate::memory::create_memory_backend;
use crate::service::WorkerService;

#[derive(Debug, Default, Serialize, Deserialize)]
struct SubconsciousState {
    #[serde(default)]
    last_tick_at: Option<DateTime<Utc>>,
}

fn state_path() -> PathBuf {
    if let Ok(p) = std::env::var("CLAWZ_SUBCONSCIOUS_STATE") {
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
    home.join("subconscious/state.json")
}

fn load_state() -> SubconsciousState {
    let path = state_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(state: &SubconsciousState) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            clawz_core::error::ClawzError::Internal(format!("subconscious state dir: {e}"))
        })?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| clawz_core::error::ClawzError::Serialization(e.to_string()))?;
    std::fs::write(&path, json).map_err(|e| {
        clawz_core::error::ClawzError::Internal(format!("write subconscious state: {e}"))
    })?;
    Ok(())
}

fn default_agent_id() -> String {
    std::env::var("CLAWZ_SUBCONSCIOUS_AGENT_ID").unwrap_or_else(|_| "default".into())
}

/// Run one subconscious reflection turn (tools disabled).
pub async fn run_subconscious_tick(
    service: &WorkerService,
    agent_id: Option<&str>,
) -> Result<(String, String, usize)> {
    let agent_id = agent_id
        .map(str::to_string)
        .unwrap_or_else(default_agent_id);
    let since = load_state().last_tick_at;
    let snippets = recent_sync_snippets(&agent_id, since.as_ref(), 8).await?;
    let chunks_reviewed = snippets.len();

    let context = if snippets.is_empty() {
        "No new integration data since the last tick.".to_string()
    } else {
        snippets
            .into_iter()
            .enumerate()
            .map(|(i, s)| format!("[{i}] {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let message = format!(
        "Subconscious review: summarize actionable insights from the following newly ingested context. \
         Be concise (bullet points). If nothing needs attention, say so.\n\n{context}"
    );

    let conversation_id = format!("subconscious-{agent_id}");
    let turn = crate::runtime::session_run::execute_agent_turn(
        service,
        &agent_id,
        RunTurnRequest {
            message,
            conversation_id: Some(conversation_id.clone()),
            background_mode: true,
            ..Default::default()
        },
    )
    .await?;

    let mut state = load_state();
    state.last_tick_at = Some(Utc::now());
    save_state(&state)?;

    Ok((conversation_id, turn.content, chunks_reviewed))
}

async fn recent_sync_snippets(
    agent_id: &str,
    since: Option<&DateTime<Utc>>,
    limit: usize,
) -> Result<Vec<String>> {
    let memory = create_memory_backend().await;
    memory.list_sync_chunks_since(agent_id, since, limit).await
}

/// Background loop unless `CLAWZ_SUBCONSCIOUS=0`. Opt-in with `CLAWZ_SUBCONSCIOUS=1`.
pub fn spawn_subconscious_scheduler(service: Arc<WorkerService>) {
    if std::env::var("CLAWZ_SUBCONSCIOUS").ok().as_deref() != Some("1") {
        tracing::info!("subconscious scheduler disabled (set CLAWZ_SUBCONSCIOUS=1 to enable)");
        return;
    }
    let interval_secs = std::env::var("CLAWZ_SUBCONSCIOUS_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        tracing::info!("subconscious scheduler started (every {interval_secs}s)");
        loop {
            interval.tick().await;
            match run_subconscious_tick(&service, None).await {
                Ok((conv, content, n)) => {
                    tracing::info!(
                        conversation_id = %conv,
                        chunks = n,
                        preview = %content.chars().take(80).collect::<String>(),
                        "subconscious tick complete"
                    );
                }
                Err(e) => tracing::warn!("subconscious tick: {e}"),
            }
        }
    });
}
