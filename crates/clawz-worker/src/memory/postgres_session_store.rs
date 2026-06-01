//! Postgres-backed [`SessionStore`](clawz_core::session::SessionStore) for micro/elastic.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;

use clawz_core::error::{ClawzError, Result};
use clawz_core::session::{SessionStore, SessionUsage};
use clawz_core::types::message::Message;

use super::session_store::FileSessionStore;

/// Enterprise session persistence; falls back to file store when the table is missing.
pub struct PostgresSessionStore {
    pool: PgPool,
    fallback: Arc<FileSessionStore>,
}

impl PostgresSessionStore {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url)
            .await
            .map_err(|e| ClawzError::Database(e.to_string()))?;
        let fallback = Arc::new(FileSessionStore::default_home());
        Ok(Self { pool, fallback })
    }

    async fn table_ready(&self) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'clawz_sessions'
            )",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }

    fn estimate_tokens(messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|m| m.content.as_text().map(|t| t.len() / 4).unwrap_or(0))
            .sum()
    }

    async fn load_json(&self, session_id: &str) -> Result<Vec<Message>> {
        if !self.table_ready().await {
            return self.fallback.load_transcript(session_id).await;
        }
        let row: Option<(Value,)> =
            sqlx::query_as("SELECT messages FROM clawz_sessions WHERE session_id = $1")
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| ClawzError::Database(e.to_string()))?;

        match row {
            Some((messages,)) => {
                let parsed: Vec<Message> = serde_json::from_value(messages)
                    .map_err(|e| ClawzError::Serialization(e.to_string()))?;
                Ok(parsed)
            }
            None => Ok(Vec::new()),
        }
    }

    async fn save_json(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        if !self.table_ready().await {
            return self.fallback.save_transcript(session_id, messages).await;
        }
        let payload =
            serde_json::to_value(messages).map_err(|e| ClawzError::Serialization(e.to_string()))?;
        sqlx::query(
            r#"
            INSERT INTO clawz_sessions (session_id, messages, updated_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (session_id) DO UPDATE
            SET messages = EXCLUDED.messages, updated_at = NOW()
            "#,
        )
        .bind(session_id)
        .bind(payload)
        .execute(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn load_transcript(&self, session_id: &str) -> Result<Vec<Message>> {
        self.load_json(session_id).await
    }

    async fn save_transcript(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        self.save_json(session_id, messages).await
    }

    async fn reset(&self, session_id: &str) -> Result<()> {
        if !self.table_ready().await {
            return self.fallback.reset(session_id).await;
        }
        sqlx::query("DELETE FROM clawz_sessions WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(())
    }

    async fn compact(&self, session_id: &str, keep_last: usize) -> Result<usize> {
        let mut messages = self.load_transcript(session_id).await?;
        if messages.len() <= keep_last {
            return Ok(0);
        }
        let removed = messages.len() - keep_last;
        messages = messages.split_off(removed);
        self.save_transcript(session_id, &messages).await?;
        Ok(removed)
    }

    async fn usage(&self, session_id: &str) -> Result<SessionUsage> {
        let messages = self.load_transcript(session_id).await?;
        Ok(SessionUsage {
            message_count: messages.len(),
            estimated_tokens: Self::estimate_tokens(&messages),
        })
    }

    async fn list_sessions(&self) -> Result<Vec<String>> {
        if !self.table_ready().await {
            return self.fallback.list_sessions().await;
        }
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT session_id FROM clawz_sessions ORDER BY updated_at DESC")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}

/// Open the best available session store for the current environment.
pub async fn open_session_store() -> Result<Arc<dyn SessionStore>> {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        match PostgresSessionStore::connect(&url).await {
            Ok(store) => {
                tracing::info!("session store: postgres (with file fallback)");
                return Ok(Arc::new(store));
            }
            Err(e) => {
                tracing::warn!("postgres session store unavailable, using file: {e}");
            }
        }
    }
    Ok(Arc::new(FileSessionStore::default_home()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate_empty() {
        assert_eq!(PostgresSessionStore::estimate_tokens(&[]), 0);
    }

    #[test]
    fn token_estimate_user_message() {
        let msgs = vec![Message::user("hello world")];
        assert!(PostgresSessionStore::estimate_tokens(&msgs) > 0);
    }
}
