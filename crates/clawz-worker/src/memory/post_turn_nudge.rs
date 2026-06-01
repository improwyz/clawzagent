//! Post-turn memory nudge — embed assistant output into the agent memory store.

use std::sync::Arc;

use clawz_core::error::Result;
use clawz_core::traits::MemoryBackend;

use super::embedding::{EmbeddingProvider, LocalEmbedding, OpenAIEmbedding};
use super::rag::RagPipeline;

/// Best-effort RAG ingest after each completed turn.
pub struct PostTurnMemoryNudge {
    enabled: bool,
}

impl PostTurnMemoryNudge {
    pub fn from_env() -> Self {
        let enabled = std::env::var("CLAWZ_MEMORY_NUDGE")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        Self { enabled }
    }

    /// Ingest the latest assistant text into vector/FTS memory when an embedder is available.
    pub async fn nudge(
        &self,
        memory: Arc<dyn MemoryBackend>,
        agent_id: &str,
        conversation_id: &str,
        assistant_text: &str,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let text = assistant_text.trim();
        if text.len() < 32 {
            return Ok(());
        }
        let Some(provider) = resolve_embedding_provider() else {
            return Ok(());
        };
        let pipeline = RagPipeline::new(memory, provider);
        let doc_id = format!("turn:{conversation_id}:{}", chrono::Utc::now().timestamp());
        let _ = pipeline.ingest(agent_id, &doc_id, text).await?;
        Ok(())
    }
}

fn resolve_embedding_provider() -> Option<Arc<dyn EmbeddingProvider>> {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.trim().is_empty() {
            return Some(Arc::new(OpenAIEmbedding::new(key)));
        }
    }
    if std::env::var("CLAWZ_OLLAMA_EMBED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        let base = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let model =
            std::env::var("CLAWZ_EMBED_MODEL").unwrap_or_else(|_| "nomic-embed-text".into());
        return Some(Arc::new(
            LocalEmbedding::new(model, 768).with_base_url(base),
        ));
    }
    None
}
