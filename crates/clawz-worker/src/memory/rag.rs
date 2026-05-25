//! RAG (Retrieval-Augmented Generation) pipeline.
//!
//! Orchestrates the two-phase RAG workflow:
//!
//! 1. **Ingestion** — chunk a document, embed each chunk, and store the vectors.
//! 2. **Retrieval** — embed a query, perform vector search, rerank candidates,
//!    and assemble a context string for LLM injection.
//!
//! // Dependency: `clawz_core::traits::MemoryBackend` for vector storage and search.

use std::sync::Arc;

use clawz_core::{
    error::Result,
    traits::{MemoryBackend, MemoryEntry},
};

// Dependency: embedding providers and text chunking live in the sibling module.
use super::embedding::{EmbeddingProvider, TextChunker};

// ── RagConfig ────────────────────────────────────────────────────────────────

/// Configuration for the RAG pipeline.
///
/// Controls thresholds, limits, and context-window budgeting.
#[derive(Debug, Clone)]
pub struct RagConfig {
    /// Minimum cosine similarity score for a retrieved chunk to be included.
    pub score_threshold: f64,
    /// Maximum number of chunks to retrieve before reranking.
    pub retrieval_limit: usize,
    /// Maximum number of chunks to return after reranking.
    pub rerank_limit: usize,
    /// Maximum total characters in the assembled context string.
    pub max_context_chars: usize,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            score_threshold: 0.35,
            retrieval_limit: 20,
            rerank_limit: 5,
            max_context_chars: 8_000,
        }
    }
}

// ── RerankedChunk ────────────────────────────────────────────────────────────

/// A retrieved memory entry decorated with reranking information.
#[derive(Debug, Clone)]
pub struct RerankedChunk {
    /// The underlying memory entry (key, value, original score, timestamp).
    pub entry: MemoryEntry,
    /// Final score after reranking (same as `entry.score` for simple cosine reranking).
    pub final_score: f64,
}

// ── RagPipeline ──────────────────────────────────────────────────────────────

/// Full RAG pipeline: ingestion and retrieval.
///
/// Holds references to a [`MemoryBackend`] and an [`EmbeddingProvider`] so
/// that documents can be chunked, embedded, stored, and later retrieved by
/// semantic similarity.
pub struct RagPipeline {
    /// Shared memory backend where chunks are persisted with embeddings.
    backend: Arc<dyn MemoryBackend>,
    /// Embedding provider used for both ingestion and query encoding.
    embedding_provider: Arc<dyn EmbeddingProvider>,
    /// Text chunker strategy (token-based with overlap).
    chunker: TextChunker,
    /// Tunable RAG thresholds and limits.
    config: RagConfig,
}

impl RagPipeline {
    /// Create a new pipeline with the given backend and embedding provider.
    ///
    /// Uses default [`RagConfig`] and [`TextChunker`].
    pub fn new(
        backend: Arc<dyn MemoryBackend>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            backend,
            embedding_provider,
            chunker: TextChunker::default(),
            config: RagConfig::default(),
        }
    }

    /// Override the default [`RagConfig`].
    pub fn with_config(mut self, config: RagConfig) -> Self {
        self.config = config;
        self
    }

    /// Override the default [`TextChunker`].
    pub fn with_chunker(mut self, chunker: TextChunker) -> Self {
        self.chunker = chunker;
        self
    }

    // ── Ingestion ─────────────────────────────────────────────────────────────

    /// Ingest a document: chunk, embed each chunk, and store all chunks.
    ///
    /// Returns the number of chunks stored.
    pub async fn ingest(
        &self,
        agent_id: &str,
        doc_id: &str,
        content: &str,
    ) -> Result<usize> {
        let chunks = self.chunker.chunk(content);
        let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();

        // Batch embed all chunks.
        let embeddings = self
            .embedding_provider
            .embed_batch(&chunk_refs)
            .await?;

        for (idx, (chunk_text, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let key = format!("{doc_id}:chunk:{idx}");
            let value = serde_json::json!({
                "doc_id": doc_id,
                "chunk_index": idx,
                "text": chunk_text,
            });
            self.backend
                .store(agent_id, &key, value, Some(embedding.clone()))
                .await?;
        }

        Ok(chunks.len())
    }

    // ── Retrieval ─────────────────────────────────────────────────────────────

    /// Retrieve the most relevant chunks for `query`, after reranking.
    pub async fn retrieve_context(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let query_embedding = self.embedding_provider.embed(query).await?;
        let candidates = self
            .backend
            .search(agent_id, query_embedding, self.config.retrieval_limit)
            .await?;

        let reranked = self.rerank(candidates, limit);
        Ok(reranked.into_iter().map(|r| r.entry).collect())
    }

    /// Retrieve context and assemble into a single context string for LLM injection.
    pub async fn assemble_context(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<String> {
        let entries = self.retrieve_context(agent_id, query, limit).await?;
        Ok(self.build_context_string(&entries))
    }

    // ── Reranking ─────────────────────────────────────────────────────────────

    /// Simple score-threshold + top-K reranking.
    ///
    /// For production workloads this can be replaced with a cross-encoder model.
    fn rerank(&self, mut candidates: Vec<MemoryEntry>, limit: usize) -> Vec<RerankedChunk> {
        // Filter below threshold.
        candidates.retain(|e| e.score >= self.config.score_threshold);

        // Sort descending by cosine score.
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top-K (respect both caller limit and config).
        let take = limit.min(self.config.rerank_limit);
        candidates.truncate(take);

        candidates
            .into_iter()
            .map(|entry| {
                let final_score = entry.score;
                RerankedChunk { entry, final_score }
            })
            .collect()
    }

    // ── Context assembly ──────────────────────────────────────────────────────

    /// Combine memory entries into a context string for LLM consumption.
    pub fn build_context_string(&self, entries: &[MemoryEntry]) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut total_chars = 0usize;

        for entry in entries {
            // Extract text from the stored JSON (supports both bare string and
            // the `{"text": "..."}` shape written by `ingest()`).
            let text = extract_text(&entry.value);
            if text.is_empty() {
                continue;
            }
            let snippet = format!("[score={:.3}] {text}", entry.score);
            total_chars += snippet.len();
            if total_chars > self.config.max_context_chars {
                break;
            }
            parts.push(snippet);
        }

        parts.join("\n\n")
    }
}

/// Extract a human-readable string from a JSON value.
///
/// Tries common text fields before falling back to the raw JSON representation.
fn extract_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            // Try common text fields.
            for field in &["text", "content", "body", "message"] {
                if let Some(serde_json::Value::String(s)) = map.get(*field) {
                    return s.clone();
                }
            }
            value.to_string()
        }
        _ => value.to_string(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::InMemoryBackend;

    struct ConstantEmbedding {
        dims: usize,
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for ConstantEmbedding {
        async fn embed(&self, _text: &str) -> clawz_core::error::Result<Vec<f32>> {
            Ok(vec![1.0_f32; self.dims])
        }
        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    fn make_pipeline() -> RagPipeline {
        let backend = Arc::new(InMemoryBackend::new());
        let provider = Arc::new(ConstantEmbedding { dims: 4 });
        RagPipeline::new(backend, provider)
    }

    #[tokio::test]
    async fn test_ingest_and_retrieve() {
        let pipeline = make_pipeline();
        let n = pipeline
            .ingest("agent-1", "doc-1", "Hello world. This is a test document.")
            .await
            .unwrap();
        assert!(n >= 1);

        let ctx = pipeline
            .retrieve_context("agent-1", "hello", 5)
            .await
            .unwrap();
        // All entries have score 1.0 (identical vectors), so all pass the threshold.
        assert!(!ctx.is_empty());
    }

    #[tokio::test]
    async fn test_assemble_context() {
        let pipeline = make_pipeline();
        pipeline
            .ingest("agent-1", "doc-1", "Hello world.")
            .await
            .unwrap();
        let context = pipeline
            .assemble_context("agent-1", "hello", 3)
            .await
            .unwrap();
        assert!(context.contains("Hello world."));
    }

    #[test]
    fn test_rerank_threshold_filtering() {
        let backend = Arc::new(InMemoryBackend::new());
        let provider = Arc::new(ConstantEmbedding { dims: 4 });
        let pipeline = RagPipeline::new(backend, provider).with_config(RagConfig {
            score_threshold: 0.8,
            retrieval_limit: 10,
            rerank_limit: 3,
            max_context_chars: 8000,
        });

        let entries = vec![
            MemoryEntry::new("k1", serde_json::json!("a"), 0.9),
            MemoryEntry::new("k2", serde_json::json!("b"), 0.5), // below threshold
            MemoryEntry::new("k3", serde_json::json!("c"), 0.85),
        ];
        let reranked = pipeline.rerank(entries, 5);
        assert_eq!(reranked.len(), 2);
        assert!(reranked[0].final_score >= reranked[1].final_score);
    }

    #[test]
    fn test_build_context_string() {
        let backend = Arc::new(InMemoryBackend::new());
        let provider = Arc::new(ConstantEmbedding { dims: 4 });
        let pipeline = RagPipeline::new(backend, provider);

        let entries = vec![
            MemoryEntry::new("k1", serde_json::json!({"text": "chunk one"}), 0.9),
            MemoryEntry::new("k2", serde_json::json!({"text": "chunk two"}), 0.7),
        ];
        let ctx = pipeline.build_context_string(&entries);
        assert!(ctx.contains("chunk one"));
        assert!(ctx.contains("chunk two"));
    }
}
