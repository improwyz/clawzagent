//! PostgresMemoryBackend and InMemoryBackend implementations of `MemoryBackend`.
//!
//! This module provides two concrete backends for the [`MemoryBackend`] trait:
//!
//! - [`PostgresMemoryBackend`] — production-grade, uses PostgreSQL + pgvector
//!   for persistent storage and approximate nearest-neighbour search.
//! - [`InMemoryBackend`] — ephemeral, HashMap-based backend useful for tests
//!   and single-user offline deployments.
//!
//! // Dependency: `clawz_core::traits::MemoryBackend` and `clawz_core::traits::MemoryEntry`

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use sqlx::{PgPool, Row};
use tokio::sync::RwLock;

// Dependency: core error type and memory trait definitions from clawz_core.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{MemoryBackend, MemoryEntry},
    types::{AgentState, Message},
};

// ── PostgresMemoryBackend ──────────────────────────────────────────────────────

/// Production-grade memory backend backed by PostgreSQL + pgvector.
///
/// Stores agent key-value pairs, conversation history, and agent state
/// in a persistent SQL database with vector similarity indexing.
pub struct PostgresMemoryBackend {
    /// Connection pool to the PostgreSQL instance.
    ///
    /// Shared across all operations; migrations run once at construction.
    pool: PgPool,
}

impl PostgresMemoryBackend {
    /// Create a new backend connected to `database_url` and run table migrations.
    ///
    /// # Errors
    ///
    /// Returns [`ClawzError::Database`] if the connection or migration fails.
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url)
            .await
            .map_err(|e| ClawzError::Database(format!("connection failed: {e}")))?;
        let backend = Self { pool };
        backend.migrate().await?;
        Ok(backend)
    }

    /// Create from an existing pool (useful for tests with shared pools).
    ///
    /// Migrations are still executed to ensure schema compatibility.
    pub async fn from_pool(pool: PgPool) -> Result<Self> {
        let backend = Self { pool };
        backend.migrate().await?;
        Ok(backend)
    }

    /// Ensure the required tables and pgvector extension exist.
    ///
    /// This is idempotent — safe to call multiple times.
    async fn migrate(&self) -> Result<()> {
        // Enable the pgvector extension; ignore if already present.
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&self.pool)
            .await
            .map_err(|e| ClawzError::Database(format!("pgvector extension: {e}")))?;

        // Main memory table: one row per (agent_id, key) with optional embedding.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS clawz_memory (
                id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                agent_id    TEXT NOT NULL,
                key         TEXT NOT NULL,
                value       JSONB NOT NULL,
                embedding   vector(1536),
                created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE (agent_id, key)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(format!("clawz_memory table: {e}")))?;

        // IVFFlat index on embeddings for fast approximate cosine similarity search.
        // Index creation may fail when the table is empty – that is fine.
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS clawz_memory_embedding_idx
            ON clawz_memory USING ivfflat (embedding vector_cosine_ops)
            WITH (lists = 100)
            "#,
        )
        .execute(&self.pool)
        .await
        // Index creation may fail when the table is empty – that is fine.
        .ok();

        // Conversation history table: append-only message log.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS clawz_conversations (
                id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                conversation_id TEXT NOT NULL,
                role            TEXT NOT NULL,
                content         JSONB NOT NULL,
                name            TEXT,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(format!("clawz_conversations table: {e}")))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS clawz_conversations_conv_id_idx ON clawz_conversations (conversation_id, created_at)"
        )
        .execute(&self.pool)
        .await
        .ok();

        Ok(())
    }

    /// Encode a `Vec<f32>` as a pgvector literal string: `[0.1,0.2,…]`.
    ///
    /// pgvector expects this bracketed comma-separated format for casting
    /// to the `vector` type in SQL queries.
    fn encode_vector(v: &[f32]) -> String {
        let inner = v
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!("[{inner}]")
    }
}

#[async_trait]
impl MemoryBackend for PostgresMemoryBackend {
    async fn store(
        &self,
        agent_id: &str,
        key: &str,
        value: Value,
        embedding: Option<Vec<f32>>,
    ) -> Result<()> {
        if let Some(emb) = embedding {
            let vec_str = Self::encode_vector(&emb);
            sqlx::query(
                r#"
                INSERT INTO clawz_memory (agent_id, key, value, embedding, updated_at)
                VALUES ($1, $2, $3, $4::vector, NOW())
                ON CONFLICT (agent_id, key) DO UPDATE
                  SET value = EXCLUDED.value,
                      embedding = EXCLUDED.embedding,
                      updated_at = NOW()
                "#,
            )
            .bind(agent_id)
            .bind(key)
            .bind(&value)
            .bind(&vec_str)
            .execute(&self.pool)
            .await
            .map_err(|e| ClawzError::Database(e.to_string()))?;
        } else {
            sqlx::query(
                r#"
                INSERT INTO clawz_memory (agent_id, key, value, updated_at)
                VALUES ($1, $2, $3, NOW())
                ON CONFLICT (agent_id, key) DO UPDATE
                  SET value = EXCLUDED.value,
                      updated_at = NOW()
                "#,
            )
            .bind(agent_id)
            .bind(key)
            .bind(&value)
            .execute(&self.pool)
            .await
            .map_err(|e| ClawzError::Database(e.to_string()))?;
        }
        Ok(())
    }

    async fn retrieve(&self, agent_id: &str, key: &str) -> Result<Option<Value>> {
        let row = sqlx::query(
            "SELECT value FROM clawz_memory WHERE agent_id = $1 AND key = $2",
        )
        .bind(agent_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;

        Ok(row.map(|r| r.get::<Value, _>("value")))
    }

    async fn search(
        &self,
        agent_id: &str,
        query_embedding: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let vec_str = Self::encode_vector(&query_embedding);
        // Use pgvector distance operator `<=>` (cosine distance).
        // Score is inverted: 1 - distance gives cosine similarity.
        let rows = sqlx::query(
            r#"
            SELECT key, value, created_at,
                   1 - (embedding <=> $3::vector) AS score
            FROM clawz_memory
            WHERE agent_id = $1
              AND embedding IS NOT NULL
            ORDER BY embedding <=> $3::vector
            LIMIT $2
            "#,
        )
        .bind(agent_id)
        .bind(limit as i64)
        .bind(&vec_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;

        let entries = rows
            .into_iter()
            .map(|row| {
                let key: String = row.get("key");
                let value: Value = row.get("value");
                let score: f64 = row.try_get("score").unwrap_or(0.0);
                let ts: chrono::DateTime<Utc> = row.get("created_at");
                MemoryEntry {
                    key,
                    value,
                    score,
                    timestamp: ts,
                }
            })
            .collect();
        Ok(entries)
    }

    async fn get_conversation_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let rows = sqlx::query(
            r#"
            SELECT role, content, name, created_at, id
            FROM clawz_conversations
            WHERE conversation_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(conversation_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;

        // Reverse so oldest is first.
        let mut messages: Vec<Message> = rows
            .into_iter()
            .map(|row| {
                let content_val: Value = row.get("content");
                serde_json::from_value(content_val)
                    .map_err(|e| ClawzError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        messages.reverse();
        Ok(messages)
    }

    async fn save_message(&self, conversation_id: &str, message: &Message) -> Result<()> {
        let content = serde_json::to_value(message)
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;
        let role = message.role.as_str();
        sqlx::query(
            r#"
            INSERT INTO clawz_conversations (conversation_id, role, content, name, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(conversation_id)
        .bind(role)
        .bind(&content)
        .bind(message.name.as_deref())
        .bind(message.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(())
    }

    async fn delete(&self, agent_id: &str, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM clawz_memory WHERE agent_id = $1 AND key = $2")
            .bind(agent_id)
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(())
    }

    async fn save_agent_state(&self, agent_id: &str, state: &AgentState) -> Result<()> {
        let value = serde_json::to_value(state)
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;
        // Use a reserved key so it does not collide with user keys.
        self.store(agent_id, "__state__", value, None).await
    }
}

// ── InMemoryBackend ───────────────────────────────────────────────────────────

/// Value tuple stored per key in the in-memory backend.
///
/// Fields: (JSON value, optional embedding vector, last-update timestamp).
type StoreValue = (Value, Option<Vec<f32>>, chrono::DateTime<Utc>);
/// Per-agent key-value map.
type AgentStore = HashMap<String, HashMap<String, StoreValue>>;

/// In-process memory backend (no database).  Suitable for testing and
/// single-user / offline deployments.
pub struct InMemoryBackend {
    /// Append-only conversation logs keyed by conversation ID.
    conversations: Arc<RwLock<HashMap<String, Vec<Message>>>>,
    /// Agent state snapshots keyed by agent ID.
    agent_states: Arc<RwLock<HashMap<String, AgentState>>>,
    /// Main key-value store with optional embeddings.
    store: Arc<RwLock<AgentStore>>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self {
            conversations: Arc::new(RwLock::new(HashMap::new())),
            agent_states: Arc::new(RwLock::new(HashMap::new())),
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Cosine similarity between two equal-length vectors.
    ///
    /// Returns 0.0 on dimension mismatch or zero-length vectors to avoid
    /// division-by-zero.
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
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryBackend for InMemoryBackend {
    async fn store(
        &self,
        agent_id: &str,
        key: &str,
        value: Value,
        embedding: Option<Vec<f32>>,
    ) -> Result<()> {
        let mut store = self.store.write().await;
        store
            .entry(agent_id.to_string())
            .or_default()
            .insert(key.to_string(), (value, embedding, Utc::now()));
        Ok(())
    }

    async fn retrieve(&self, agent_id: &str, key: &str) -> Result<Option<Value>> {
        let store = self.store.read().await;
        Ok(store
            .get(agent_id)
            .and_then(|m| m.get(key))
            .map(|(v, _, _)| v.clone()))
    }

    async fn search(
        &self,
        agent_id: &str,
        query_embedding: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let store = self.store.read().await;
        let agent_map = match store.get(agent_id) {
            Some(m) => m,
            None => return Ok(vec![]),
        };

        let mut scored: Vec<(f64, String, Value, chrono::DateTime<Utc>)> = agent_map
            .iter()
            .map(|(k, (v, emb, ts))| {
                let score = match emb {
                    Some(e) => Self::cosine_similarity(&query_embedding, e),
                    None => 0.0,
                };
                (score, k.clone(), v.clone(), *ts)
            })
            .collect();

        // Sort by descending score.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .map(|(score, key, value, timestamp)| MemoryEntry {
                key,
                value,
                score,
                timestamp,
            })
            .collect())
    }

    async fn get_conversation_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let conversations = self.conversations.read().await;
        let history = conversations
            .get(conversation_id)
            .cloned()
            .unwrap_or_default();
        let start = history.len().saturating_sub(limit);
        Ok(history[start..].to_vec())
    }

    async fn save_message(&self, conversation_id: &str, message: &Message) -> Result<()> {
        let mut conversations = self.conversations.write().await;
        conversations
            .entry(conversation_id.to_string())
            .or_default()
            .push(message.clone());
        Ok(())
    }

    async fn delete(&self, agent_id: &str, key: &str) -> Result<()> {
        let mut store = self.store.write().await;
        if let Some(m) = store.get_mut(agent_id) {
            m.remove(key);
        }
        Ok(())
    }

    async fn save_agent_state(&self, agent_id: &str, state: &AgentState) -> Result<()> {
        let mut agent_states = self.agent_states.write().await;
        agent_states.insert(agent_id.to_string(), state.clone());
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::{AgentStatus, Message};

    #[tokio::test]
    async fn test_inmemory_store_retrieve() {
        let backend = InMemoryBackend::new();
        backend
            .store("agent-1", "key-1", serde_json::json!({"x": 1}), None)
            .await
            .unwrap();
        let val = backend.retrieve("agent-1", "key-1").await.unwrap();
        assert_eq!(val, Some(serde_json::json!({"x": 1})));
    }

    #[tokio::test]
    async fn test_inmemory_delete() {
        let backend = InMemoryBackend::new();
        backend
            .store("a", "k", serde_json::json!(1), None)
            .await
            .unwrap();
        backend.delete("a", "k").await.unwrap();
        assert!(backend.retrieve("a", "k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_inmemory_conversation() {
        let backend = InMemoryBackend::new();
        let msg = Message::user("hello");
        backend.save_message("conv-1", &msg).await.unwrap();
        let hist = backend
            .get_conversation_history("conv-1", 10)
            .await
            .unwrap();
        assert_eq!(hist.len(), 1);
    }

    #[tokio::test]
    async fn test_inmemory_search_with_embeddings() {
        let backend = InMemoryBackend::new();
        backend
            .store("a", "k1", serde_json::json!("v1"), Some(vec![1.0, 0.0]))
            .await
            .unwrap();
        backend
            .store("a", "k2", serde_json::json!("v2"), Some(vec![0.0, 1.0]))
            .await
            .unwrap();
        let results = backend
            .search("a", vec![1.0, 0.0], 2)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].key, "k1");
        assert!((results[0].score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_inmemory_save_agent_state() {
        let backend = InMemoryBackend::new();
        let state = AgentState {
            status: AgentStatus::Running,
            ..Default::default()
        };
        backend.save_agent_state("agent-1", &state).await.unwrap();
    }
}
