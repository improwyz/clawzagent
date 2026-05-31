//! SQLite + FTS5 memory backend for standalone / desktop deployments.
//!
//! Persists agent key-value memory and conversation history under
//! `~/.clawz/memory.db` (override with `CLAWZ_MEMORY_DB`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde_json::Value;
use tokio::task;

use clawz_core::{
    error::{ClawzError, Result},
    traits::{MemoryBackend, MemoryEntry},
    types::{agent::AgentState, message::Message},
};

/// Default on-disk path relative to [`default_db_path`].
pub const DEFAULT_MEMORY_DB: &str = "memory.db";

/// File-backed memory with FTS5 full-text search.
#[derive(Debug, Clone)]
pub struct SqliteMemoryBackend {
    db_path: PathBuf,
}

impl SqliteMemoryBackend {
    /// Open (or create) the database at `path` and run migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ClawzError::Database(format!("create memory db dir: {e}")))?;
        }
        let path_clone = db_path.clone();
        task::spawn_blocking(move || Self::migrate(&path_clone))
            .await
            .map_err(|e| ClawzError::Internal(format!("sqlite migrate join: {e}")))??;
        Ok(Self { db_path })
    }

    /// `CLAWZ_MEMORY_DB` or `$CLAWZ_HOME/memory.db` or `~/.clawz/memory.db`.
    pub fn default_db_path() -> PathBuf {
        if let Ok(p) = std::env::var("CLAWZ_MEMORY_DB") {
            return PathBuf::from(p);
        }
        let home = std::env::var("CLAWZ_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".clawz")
            });
        home.join(DEFAULT_MEMORY_DB)
    }

    /// Open the default personal memory database.
    pub async fn open_default() -> Result<Self> {
        Self::open(Self::default_db_path()).await
    }

    /// List `sync:*` memory chunks updated after `since` (for subconscious ticks).
    pub async fn list_sync_chunks_since(
        &self,
        agent_id: &str,
        since: Option<&DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let agent_id = agent_id.to_string();
        let since_str = since.map(|t| t.to_rfc3339());
        self.with_conn(move |conn| {
            let mut snippets = Vec::new();
            if let Some(since) = since_str {
                let mut stmt = conn
                    .prepare(
                        r#"
                        SELECT value FROM clawz_memory
                        WHERE agent_id = ?1 AND key LIKE 'sync:%' AND updated_at > ?2
                        ORDER BY updated_at DESC
                        LIMIT ?3
                        "#,
                    )
                    .map_err(|e| ClawzError::Database(e.to_string()))?;
                let rows = stmt
                    .query_map(params![agent_id, since, limit as i64], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| ClawzError::Database(e.to_string()))?;
                for row in rows {
                    let raw = row.map_err(|e| ClawzError::Database(e.to_string()))?;
                    if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                        if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
                            snippets.push(text.to_string());
                        }
                    }
                }
            } else {
                let mut stmt = conn
                    .prepare(
                        r#"
                        SELECT value FROM clawz_memory
                        WHERE agent_id = ?1 AND key LIKE 'sync:%'
                        ORDER BY updated_at DESC
                        LIMIT ?2
                        "#,
                    )
                    .map_err(|e| ClawzError::Database(e.to_string()))?;
                let rows = stmt
                    .query_map(params![agent_id, limit as i64], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| ClawzError::Database(e.to_string()))?;
                for row in rows {
                    let raw = row.map_err(|e| ClawzError::Database(e.to_string()))?;
                    if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                        if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
                            snippets.push(text.to_string());
                        }
                    }
                }
            }
            Ok(snippets)
        })
        .await
    }

    fn migrate(path: &Path) -> Result<()> {
        let conn = Connection::open(path)
            .map_err(|e| ClawzError::Database(format!("sqlite open: {e}")))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS clawz_memory (
                agent_id    TEXT NOT NULL,
                key         TEXT NOT NULL,
                value       TEXT NOT NULL,
                embedding   BLOB,
                updated_at  TEXT NOT NULL,
                PRIMARY KEY (agent_id, key)
            );

            CREATE TABLE IF NOT EXISTS clawz_conversations (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                role            TEXT NOT NULL,
                content         TEXT NOT NULL,
                name            TEXT,
                created_at      TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS clawz_conversations_conv_idx
                ON clawz_conversations (conversation_id, created_at);

            CREATE VIRTUAL TABLE IF NOT EXISTS clawz_memory_fts USING fts5(
                agent_id UNINDEXED,
                memory_key UNINDEXED,
                body,
                tokenize = 'unicode61'
            );
            "#,
        )
        .map_err(|e| ClawzError::Database(format!("sqlite migrate: {e}")))?;
        Ok(())
    }

    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let path = self.db_path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(&path)
                .map_err(|e| ClawzError::Database(format!("sqlite open: {e}")))?;
            f(&conn)
        })
        .await
        .map_err(|e| ClawzError::Internal(format!("sqlite task: {e}")))?
    }

    fn encode_embedding(v: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(v.len() * 4);
        for f in v {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes
    }

    fn decode_embedding(bytes: &[u8]) -> Option<Vec<f32>> {
        if bytes.len() % 4 != 0 {
            return None;
        }
        Some(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }

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

    fn index_memory_fts(conn: &Connection, agent_id: &str, key: &str, body: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM clawz_memory_fts WHERE agent_id = ?1 AND memory_key = ?2",
            params![agent_id, key],
        )
        .map_err(|e| ClawzError::Database(e.to_string()))?;
        if body.trim().is_empty() {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO clawz_memory_fts (agent_id, memory_key, body) VALUES (?1, ?2, ?3)",
            params![agent_id, key, body],
        )
        .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(())
    }

    fn message_search_text(message: &Message) -> String {
        serde_json::to_string(&message.content).unwrap_or_default()
    }

    fn value_search_text(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            _ => value.to_string(),
        }
    }
}

fn fts_query(raw: &str) -> String {
    let terms: Vec<String> = raw
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            let escaped = t.replace('"', "");
            format!("\"{escaped}\"")
        })
        .collect();
    terms.join(" OR ")
}

fn parse_timestamp(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[async_trait]
impl MemoryBackend for SqliteMemoryBackend {
    async fn store(
        &self,
        agent_id: &str,
        key: &str,
        value: Value,
        embedding: Option<Vec<f32>>,
    ) -> Result<()> {
        let agent_id = agent_id.to_string();
        let key = key.to_string();
        let value_json =
            serde_json::to_string(&value).map_err(|e| ClawzError::Serialization(e.to_string()))?;
        let emb_blob = embedding.as_ref().map(|e| Self::encode_embedding(e));
        let body = Self::value_search_text(&value);
        let updated_at = Utc::now().to_rfc3339();

        self.with_conn(move |conn| {
            conn.execute(
                r#"
                INSERT INTO clawz_memory (agent_id, key, value, embedding, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(agent_id, key) DO UPDATE SET
                    value = excluded.value,
                    embedding = excluded.embedding,
                    updated_at = excluded.updated_at
                "#,
                params![agent_id, key, value_json, emb_blob, updated_at],
            )
            .map_err(|e| ClawzError::Database(e.to_string()))?;
            Self::index_memory_fts(conn, &agent_id, &key, &body)
        })
        .await
    }

    async fn retrieve(&self, agent_id: &str, key: &str) -> Result<Option<Value>> {
        let agent_id = agent_id.to_string();
        let key = key.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT value FROM clawz_memory WHERE agent_id = ?1 AND key = ?2")
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let mut rows = stmt
                .query(params![agent_id, key])
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            if let Some(row) = rows
                .next()
                .map_err(|e| ClawzError::Database(e.to_string()))?
            {
                let raw: String = row
                    .get(0)
                    .map_err(|e| ClawzError::Database(e.to_string()))?;
                let val: Value = serde_json::from_str(&raw)
                    .map_err(|e| ClawzError::Serialization(e.to_string()))?;
                Ok(Some(val))
            } else {
                Ok(None)
            }
        })
        .await
    }

    async fn search(
        &self,
        agent_id: &str,
        query_embedding: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let agent_id = agent_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT key, value, embedding, updated_at FROM clawz_memory WHERE agent_id = ?1",
                )
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![agent_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| ClawzError::Database(e.to_string()))?;

            let mut scored = Vec::new();
            for row in rows {
                let (key, value_raw, emb_blob, updated_at) =
                    row.map_err(|e| ClawzError::Database(e.to_string()))?;
                let score = emb_blob
                    .as_deref()
                    .and_then(Self::decode_embedding)
                    .map(|emb| Self::cosine_similarity(&query_embedding, &emb))
                    .unwrap_or(0.0);
                let value: Value = serde_json::from_str(&value_raw)
                    .map_err(|e| ClawzError::Serialization(e.to_string()))?;
                scored.push(MemoryEntry {
                    key,
                    value,
                    score,
                    timestamp: parse_timestamp(&updated_at),
                });
            }
            scored.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            scored.truncate(limit);
            Ok(scored)
        })
        .await
    }

    async fn list_sync_chunks_since(
        &self,
        agent_id: &str,
        since: Option<&DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.list_sync_chunks_since(agent_id, since, limit).await
    }

    async fn search_text(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let fts = fts_query(query);
        if fts.is_empty() {
            return Ok(vec![]);
        }
        let agent_id = agent_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT memory_key, body
                    FROM clawz_memory_fts
                    WHERE clawz_memory_fts MATCH ?1 AND agent_id = ?2
                    LIMIT ?3
                    "#,
                )
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![fts, agent_id, limit as i64], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| ClawzError::Database(e.to_string()))?;

            let mut out = Vec::new();
            for row in rows {
                let (key, body) = row.map_err(|e| ClawzError::Database(e.to_string()))?;
                out.push(MemoryEntry {
                    key: key.clone(),
                    value: Value::String(body),
                    score: 1.0,
                    timestamp: Utc::now(),
                });
            }
            Ok(out)
        })
        .await
    }

    async fn get_conversation_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let conversation_id = conversation_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT role, content, name, created_at
                    FROM clawz_conversations
                    WHERE conversation_id = ?1
                    ORDER BY created_at ASC
                    "#,
                )
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![conversation_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| ClawzError::Database(e.to_string()))?;

            let mut messages = Vec::new();
            for row in rows {
                let (role, content, name, created_at) =
                    row.map_err(|e| ClawzError::Database(e.to_string()))?;
                let role = match role.as_str() {
                    "user" => clawz_core::types::message::Role::User,
                    "assistant" => clawz_core::types::message::Role::Assistant,
                    "system" => clawz_core::types::message::Role::System,
                    "tool" => clawz_core::types::message::Role::Tool,
                    _ => clawz_core::types::message::Role::User,
                };
                let content: clawz_core::types::message::MessageContent =
                    serde_json::from_str(&content)
                        .map_err(|e| ClawzError::Serialization(e.to_string()))?;
                messages.push(Message {
                    id: uuid::Uuid::new_v4(),
                    role,
                    content,
                    created_at: parse_timestamp(&created_at),
                    name,
                });
            }
            let start = messages.len().saturating_sub(limit);
            Ok(messages[start..].to_vec())
        })
        .await
    }

    async fn save_message(&self, conversation_id: &str, message: &Message) -> Result<()> {
        let conversation_id = conversation_id.to_string();
        let message = message.clone();
        self.with_conn(move |conn| {
            let content = serde_json::to_string(&message.content)
                .map_err(|e| ClawzError::Serialization(e.to_string()))?;
            let created_at = message.created_at.to_rfc3339();
            conn.execute(
                r#"
                INSERT INTO clawz_conversations (conversation_id, role, content, name, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![
                    conversation_id,
                    message.role.as_str(),
                    content,
                    message.name,
                    created_at,
                ],
            )
            .map_err(|e| ClawzError::Database(e.to_string()))?;

            let body = Self::message_search_text(&message);
            let key = format!("conv:{}", message.id);
            Self::index_memory_fts(conn, "conversations", &key, &body)?;
            Ok(())
        })
        .await
    }

    async fn delete(&self, agent_id: &str, key: &str) -> Result<()> {
        let agent_id = agent_id.to_string();
        let key = key.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM clawz_memory WHERE agent_id = ?1 AND key = ?2",
                params![agent_id, key],
            )
            .map_err(|e| ClawzError::Database(e.to_string()))?;
            conn.execute(
                "DELETE FROM clawz_memory_fts WHERE agent_id = ?1 AND memory_key = ?2",
                params![agent_id, key],
            )
            .map_err(|e| ClawzError::Database(e.to_string()))?;
            Ok(())
        })
        .await
    }

    async fn save_agent_state(&self, agent_id: &str, state: &AgentState) -> Result<()> {
        let value =
            serde_json::to_value(state).map_err(|e| ClawzError::Serialization(e.to_string()))?;
        self.store(agent_id, "__state__", value, None).await
    }
}

/// Resolve the best memory backend for the current deployment.
pub async fn create_memory_backend() -> Arc<dyn MemoryBackend> {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        if !url.trim().is_empty() {
            match super::PostgresMemoryBackend::new(&url).await {
                Ok(backend) => {
                    tracing::info!("memory backend: postgres");
                    return Arc::new(backend);
                }
                Err(e) => {
                    tracing::warn!("Postgres memory unavailable ({e}), trying sqlite");
                }
            }
        }
    }

    let force_sqlite = std::env::var("CLAWZ_SQLITE_MEMORY").is_ok();
    let standalone = clawz_core::deployment::DeploymentMode::from_env()
        == clawz_core::deployment::DeploymentMode::Standalone;

    if force_sqlite || standalone {
        match SqliteMemoryBackend::open_default().await {
            Ok(backend) => {
                tracing::info!("memory backend: sqlite ({})", backend.db_path.display());
                return Arc::new(backend);
            }
            Err(e) => {
                tracing::warn!("SQLite memory unavailable ({e}), using in-memory");
            }
        }
    }

    tracing::info!("memory backend: in-memory");
    Arc::new(super::InMemoryBackend::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::message::Message;

    #[tokio::test]
    async fn sqlite_store_retrieve_and_fts() {
        let dir = std::env::temp_dir().join(format!("clawz-sqlite-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.db");
        let backend = SqliteMemoryBackend::open(&path).await.unwrap();

        backend
            .store(
                "agent-1",
                "note",
                serde_json::json!("rust async patterns"),
                None,
            )
            .await
            .unwrap();

        let val = backend.retrieve("agent-1", "note").await.unwrap();
        assert_eq!(val, Some(serde_json::json!("rust async patterns")));

        let hits = backend
            .search_text("agent-1", "async rust", 5)
            .await
            .unwrap();
        assert!(!hits.is_empty());

        let msg = Message::user("hello sqlite");
        backend.save_message("conv-1", &msg).await.unwrap();
        let hist = backend
            .get_conversation_history("conv-1", 10)
            .await
            .unwrap();
        assert_eq!(hist.len(), 1);

        let _ = std::fs::remove_dir_all(dir);
    }
}
