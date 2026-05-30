//! Store connector-sync chunks into the agent memory backend.

use serde::{Deserialize, Serialize};
use serde_json::json;

use clawz_core::error::Result;

use crate::memory::create_memory_backend;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryIngestChunk {
    pub key: String,
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
}

/// Persist text chunks under `sync:{key}` for FTS retrieval and subconscious review.
pub async fn ingest_memory_chunks(agent_id: &str, chunks: Vec<MemoryIngestChunk>) -> Result<usize> {
    if chunks.is_empty() {
        return Ok(0);
    }
    let memory = create_memory_backend().await;
    let mut stored = 0usize;
    for chunk in chunks {
        let mem_key = if chunk.key.starts_with("sync:") {
            chunk.key.clone()
        } else {
            format!("sync:{}", chunk.key)
        };
        let value = json!({
            "text": chunk.text,
            "source": chunk.source,
            "ingested_at": chrono::Utc::now().to_rfc3339(),
        });
        memory.store(agent_id, &mem_key, value, None).await?;
        stored += 1;
    }
    Ok(stored)
}
