//! Conversation history management: append-only log, summarisation, and
//! semantic search across conversations.
//!
//! [`ConversationStore`] keeps messages in memory and supports optional
//! summarisation of old turns to bound context-window growth. When an
//! embedding provider is configured, it can also perform semantic search
//! across conversation summaries.
//!
//! // Dependency: `clawz_core::traits::MemoryEntry` for cross-conversation search results.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use clawz_core::{
    error::{ClawzError, Result},
    traits::MemoryEntry,
    types::{Message, MessageContent},
};

// Dependency: embedding provider from the sibling module for semantic search.
use super::embedding::EmbeddingProvider;

// ── ConversationTurn ──────────────────────────────────────────────────────────

/// A single turn in a conversation (user + assistant pair).
#[derive(Debug, Clone)]
pub struct ConversationTurn {
    /// Unique conversation identifier.
    pub conversation_id: String,
    /// The user's message in this turn.
    pub user_message: Message,
    /// The assistant's reply in this turn.
    pub assistant_message: Message,
    /// Zero-based turn index within the conversation.
    pub turn_index: usize,
}

// ── ConversationSummary ───────────────────────────────────────────────────────

/// Compressed representation of older conversation turns.
#[derive(Debug, Clone)]
pub struct ConversationSummary {
    /// Identifier of the conversation this summary belongs to.
    pub conversation_id: String,
    /// Plain-text summary generated from old turns.
    pub summary_text: String,
    /// Number of original turns that were compressed.
    pub turn_count: usize,
    /// When the summary was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Optional embedding vector for semantic search.
    pub embedding: Option<Vec<f32>>,
}

// ── Inner state ───────────────────────────────────────────────────────────────

/// Private mutable state for a single conversation.
struct ConversationData {
    /// Raw message log (may be truncated after summarisation).
    messages: Vec<Message>,
    /// Optional compressed summary of older turns.
    summary: Option<ConversationSummary>,
}

impl ConversationData {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            summary: None,
        }
    }
}

// ── ConversationStore ─────────────────────────────────────────────────────────

/// In-memory append-only conversation store with optional semantic search.
///
/// Messages are stored per-conversation in a [`HashMap`] protected by a
/// [`RwLock`]. Summarisation discards old messages, so the live message list
/// stays bounded. If an [`EmbeddingProvider`] is supplied, summaries are
/// embedded and can be searched across conversations.
pub struct ConversationStore {
    /// Per-conversation message data.
    conversations: Arc<RwLock<HashMap<String, ConversationData>>>,
    /// Optional provider for generating summary embeddings.
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

impl ConversationStore {
    pub fn new() -> Self {
        Self {
            conversations: Arc::new(RwLock::new(HashMap::new())),
            embedding_provider: None,
        }
    }

    /// Attach an embedding provider to enable semantic search.
    pub fn with_embedding_provider(mut self, provider: Arc<dyn EmbeddingProvider>) -> Self {
        self.embedding_provider = Some(provider);
        self
    }

    // ── Appending messages ────────────────────────────────────────────────────

    /// Append a single message to a conversation.
    pub async fn append(&self, conversation_id: &str, message: Message) -> Result<()> {
        let mut guard = self.conversations.write().await;
        guard
            .entry(conversation_id.to_string())
            .or_insert_with(ConversationData::new)
            .messages
            .push(message);
        Ok(())
    }

    /// Save a complete user+assistant turn atomically.
    pub async fn save_turn(
        &self,
        conversation_id: &str,
        user_msg: Message,
        assistant_msg: Message,
    ) -> Result<()> {
        let mut guard = self.conversations.write().await;
        let data = guard
            .entry(conversation_id.to_string())
            .or_insert_with(ConversationData::new);
        data.messages.push(user_msg);
        data.messages.push(assistant_msg);
        Ok(())
    }

    // ── Retrieving history ────────────────────────────────────────────────────

    /// Return the most recent `limit` messages in chronological order.
    pub async fn get_history(&self, conversation_id: &str, limit: usize) -> Result<Vec<Message>> {
        let guard = self.conversations.read().await;
        let msgs = guard
            .get(conversation_id)
            .map(|d| d.messages.clone())
            .unwrap_or_default();
        let start = msgs.len().saturating_sub(limit);
        Ok(msgs[start..].to_vec())
    }

    /// Return all messages for a conversation.
    pub async fn get_all(&self, conversation_id: &str) -> Result<Vec<Message>> {
        let guard = self.conversations.read().await;
        Ok(guard
            .get(conversation_id)
            .map(|d| d.messages.clone())
            .unwrap_or_default())
    }

    // ── Summarisation ─────────────────────────────────────────────────────────

    /// Compress the oldest messages in a conversation into a plain-text summary,
    /// retaining only the `keep_recent` most recent messages as live context.
    ///
    /// The summary is stored on the `ConversationData` and prepended as a
    /// synthetic `System` message when `get_history` is called.
    pub async fn summarize_old(
        &self,
        conversation_id: &str,
        keep_recent: usize,
    ) -> Result<Option<ConversationSummary>> {
        let mut guard = self.conversations.write().await;
        let data = match guard.get_mut(conversation_id) {
            Some(d) => d,
            None => return Ok(None),
        };

        let total = data.messages.len();
        if total <= keep_recent {
            // Nothing to summarise.
            return Ok(data.summary.clone());
        }

        let old_end = total - keep_recent;
        let to_summarise = &data.messages[..old_end];

        // Build a plain-text representation of the old turns.
        let mut summary_text = String::new();
        for msg in to_summarise {
            let role = msg.role.as_str();
            let text = match &msg.content {
                MessageContent::Text(t) => t.clone(),
                _ => "[non-text message]".into(),
            };
            summary_text.push_str(&format!("{role}: {text}\n"));
        }
        let summary_text = format!("[SUMMARY OF EARLIER CONVERSATION]\n{summary_text}");

        // Optionally embed the summary for semantic search.
        let embedding = if let Some(ep) = &self.embedding_provider {
            Some(ep.embed(&summary_text).await?)
        } else {
            None
        };

        let summary = ConversationSummary {
            conversation_id: conversation_id.to_string(),
            summary_text: summary_text.clone(),
            turn_count: old_end,
            created_at: chrono::Utc::now(),
            embedding,
        };
        data.summary = Some(summary.clone());

        // Discard the old messages; keep only the recent tail.
        data.messages = data.messages[old_end..].to_vec();

        Ok(Some(summary))
    }

    // ── Semantic search across conversations ──────────────────────────────────

    /// Search across all stored conversations using a precomputed query embedding.
    ///
    /// Returns `MemoryEntry` values whose `value` is a JSON object with
    /// `conversation_id` and `text` fields.
    pub async fn search_conversations(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let guard = self.conversations.read().await;

        let mut scored: Vec<MemoryEntry> = Vec::new();

        for (conv_id, data) in guard.iter() {
            // Include the summary if it has an embedding.
            if let Some(summary) = &data.summary {
                if let Some(emb) = &summary.embedding {
                    let score = cosine_similarity(query_embedding, emb);
                    scored.push(MemoryEntry {
                        key: format!("{conv_id}:summary"),
                        value: serde_json::json!({
                            "conversation_id": conv_id,
                            "text": summary.summary_text,
                            "kind": "summary",
                        }),
                        score,
                        timestamp: summary.created_at,
                    });
                }
            }
        }

        // Sort descending by score.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    /// Embed `query` and then call `search_conversations`.
    pub async fn search_conversations_by_query(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let provider = self.embedding_provider.as_ref().ok_or_else(|| {
            ClawzError::Memory("no embedding provider configured for conversation search".into())
        })?;
        let embedding = provider.embed(query).await?;
        self.search_conversations(&embedding, limit).await
    }
}

impl Default for ConversationStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Cosine similarity between two equal-length vectors (returns 0.0 on mismatch).
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)) as f64
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::Message;

    #[tokio::test]
    async fn test_append_and_get_history() {
        let store = ConversationStore::new();
        store
            .append("conv-1", Message::user("hello"))
            .await
            .unwrap();
        store
            .append("conv-1", Message::assistant("world"))
            .await
            .unwrap();
        let hist = store.get_history("conv-1", 10).await.unwrap();
        assert_eq!(hist.len(), 2);
    }

    #[tokio::test]
    async fn test_get_history_limit() {
        let store = ConversationStore::new();
        for i in 0..10 {
            store
                .append("conv-1", Message::user(format!("msg {i}")))
                .await
                .unwrap();
        }
        let hist = store.get_history("conv-1", 3).await.unwrap();
        assert_eq!(hist.len(), 3);
    }

    #[tokio::test]
    async fn test_save_turn() {
        let store = ConversationStore::new();
        store
            .save_turn("conv-1", Message::user("hi"), Message::assistant("hey"))
            .await
            .unwrap();
        let hist = store.get_all("conv-1").await.unwrap();
        assert_eq!(hist.len(), 2);
    }

    #[tokio::test]
    async fn test_summarize_old() {
        let store = ConversationStore::new();
        for i in 0..8 {
            store
                .append("conv-1", Message::user(format!("msg {i}")))
                .await
                .unwrap();
        }
        let summary = store.summarize_old("conv-1", 3).await.unwrap();
        assert!(summary.is_some());
        let summary = summary.unwrap();
        assert_eq!(summary.turn_count, 5);

        // After summarization, only 3 messages remain.
        let remaining = store.get_all("conv-1").await.unwrap();
        assert_eq!(remaining.len(), 3);
    }

    #[tokio::test]
    async fn test_summarize_no_op_when_short() {
        let store = ConversationStore::new();
        store.append("conv-1", Message::user("hi")).await.unwrap();
        let result = store.summarize_old("conv-1", 5).await.unwrap();
        // Nothing to summarize.
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_empty_conversation() {
        let store = ConversationStore::new();
        let hist = store.get_history("no-such-conv", 10).await.unwrap();
        assert!(hist.is_empty());
    }
}
