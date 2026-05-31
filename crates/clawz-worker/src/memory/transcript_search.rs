//! Cross-session FTS index over JSONL session transcripts.

use std::path::{Path, PathBuf};

use clawz_core::error::{ClawzError, Result};
use clawz_core::types::message::{Message, Role};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::task;

/// A single FTS hit across session transcripts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptHit {
    pub session_id: String,
    pub role: String,
    pub snippet: String,
    pub score: f64,
}

/// SQLite FTS5 index over all session transcript lines.
pub struct TranscriptFtsIndex {
    db_path: PathBuf,
}

impl TranscriptFtsIndex {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ClawzError::Database(format!("create transcript fts dir: {e}")))?;
        }
        let path_clone = db_path.clone();
        task::spawn_blocking(move || Self::migrate(&path_clone))
            .await
            .map_err(|e| ClawzError::Internal(format!("transcript fts migrate join: {e}")))??;
        Ok(Self { db_path })
    }

    pub fn default_db_path() -> PathBuf {
        let home = std::env::var("CLAWZ_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/tmp/clawz"))
                    .join(".clawz")
            });
        home.join("transcript_fts.db")
    }

    pub async fn open_default() -> Result<Self> {
        Self::open(Self::default_db_path()).await
    }

    fn migrate(path: &Path) -> Result<()> {
        let conn = Connection::open(path)
            .map_err(|e| ClawzError::Database(format!("transcript fts open: {e}")))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE VIRTUAL TABLE IF NOT EXISTS transcript_fts USING fts5(
                session_id UNINDEXED,
                role UNINDEXED,
                body,
                tokenize = 'unicode61'
            );
            "#,
        )
        .map_err(|e| ClawzError::Database(format!("transcript fts migrate: {e}")))?;
        Ok(())
    }

    /// Re-index all non-system messages for a session.
    pub async fn index_session(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        let session_id = session_id.to_string();
        let rows: Vec<(String, String, String)> = messages
            .iter()
            .filter_map(|m| {
                let text = m.content.as_text()?;
                if text.trim().is_empty() {
                    return None;
                }
                let role = match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System => return None,
                    Role::Tool => "tool",
                };
                Some((session_id.clone(), role.to_string(), text.to_string()))
            })
            .collect();

        let db_path = self.db_path.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)
                .map_err(|e| ClawzError::Database(format!("transcript fts open: {e}")))?;
            conn.execute(
                "DELETE FROM transcript_fts WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|e| ClawzError::Database(e.to_string()))?;
            for (sid, role, body) in rows {
                conn.execute(
                    "INSERT INTO transcript_fts (session_id, role, body) VALUES (?1, ?2, ?3)",
                    params![sid, role, body],
                )
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            }
            Ok::<(), ClawzError>(())
        })
        .await
        .map_err(|e| ClawzError::Internal(format!("transcript fts index join: {e}")))??;
        Ok(())
    }

    /// Search all indexed transcripts.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<TranscriptHit>> {
        let fts = fts_query(query);
        if fts.is_empty() {
            return Ok(vec![]);
        }
        let db_path = self.db_path.clone();
        let limit = limit.clamp(1, 50) as i64;
        task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)
                .map_err(|e| ClawzError::Database(format!("transcript fts open: {e}")))?;
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT session_id, role, snippet(transcript_fts, 2, '<b>', '</b>', '…', 32) AS snip,
                           bm25(transcript_fts) AS score
                    FROM transcript_fts
                    WHERE transcript_fts MATCH ?1
                    ORDER BY score
                    LIMIT ?2
                    "#,
                )
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![fts, limit], |row| {
                    Ok(TranscriptHit {
                        session_id: row.get(0)?,
                        role: row.get(1)?,
                        snippet: row.get(2)?,
                        score: row.get(3)?,
                    })
                })
                .map_err(|e| ClawzError::Database(e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| ClawzError::Database(e.to_string()))?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| ClawzError::Internal(format!("transcript fts search join: {e}")))?
    }
}

fn fts_query(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            let escaped = t.replace('"', "");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::message::Message;

    #[tokio::test]
    async fn transcript_fts_roundtrip() {
        let path = std::env::temp_dir().join(format!("clawz-fts-{}", uuid::Uuid::new_v4()));
        let index = TranscriptFtsIndex::open(path.join("fts.db")).await.unwrap();
        let session = "sess-1";
        index
            .index_session(
                session,
                &[
                    Message::user("deploy kubernetes cluster"),
                    Message::assistant("Here is the plan for k8s"),
                ],
            )
            .await
            .unwrap();

        let hits = index.search("kubernetes", 5).await.unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].session_id, session);
    }
}
