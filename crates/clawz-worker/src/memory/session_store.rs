//! Filesystem-backed [`SessionStore`](clawz_core::session::SessionStore) for standalone mode.
//!
//! Transcripts live under `~/.clawz/sessions/{session_id}/transcript.jsonl`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use clawz_core::session::{SessionStore, SessionUsage};
use clawz_core::types::message::Message;
use tokio::io::AsyncWriteExt;

/// Local JSONL transcript store.
pub struct FileSessionStore {
    root: PathBuf,
}

impl FileSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_home() -> Self {
        let home = std::env::var("CLAWZ_HOME").unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| format!("{h}/.clawz"))
                .unwrap_or_else(|_| "/tmp/clawz".to_string())
        });
        Self::new(PathBuf::from(home).join("sessions"))
    }

    fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.root
            .join(sanitize_session_id(session_id))
            .join("transcript.jsonl")
    }

    async fn ensure_parent(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ClawzError::Internal(format!("create session dir: {e}")))?;
        }
        Ok(())
    }
}

fn sanitize_session_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter_map(|m| m.content.as_text())
        .map(|t| t.len() / 4 + 1)
        .sum()
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn load_transcript(&self, session_id: &str) -> Result<Vec<Message>> {
        let path = self.transcript_path(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ClawzError::Internal(format!("read transcript: {e}")))?;
        let mut messages = Vec::new();
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let msg: Message = serde_json::from_str(line)
                .map_err(|e| ClawzError::Serialization(format!("transcript line: {e}")))?;
            messages.push(msg);
        }
        Ok(messages)
    }

    async fn save_transcript(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        let path = self.transcript_path(session_id);
        self.ensure_parent(&path).await?;
        let mut body = String::new();
        for msg in messages {
            let line =
                serde_json::to_string(msg).map_err(|e| ClawzError::Serialization(e.to_string()))?;
            body.push_str(&line);
            body.push('\n');
        }
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| ClawzError::Internal(format!("write transcript: {e}")))?;
        file.write_all(body.as_bytes())
            .await
            .map_err(|e| ClawzError::Internal(format!("write transcript: {e}")))?;
        file.flush()
            .await
            .map_err(|e| ClawzError::Internal(format!("flush transcript: {e}")))?;
        Ok(())
    }

    async fn reset(&self, session_id: &str) -> Result<()> {
        let path = self.transcript_path(session_id);
        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| ClawzError::Internal(format!("reset session: {e}")))?;
        }
        Ok(())
    }

    async fn compact(&self, session_id: &str, keep_last: usize) -> Result<usize> {
        let keep = keep_last.max(1);
        let messages = self.load_transcript(session_id).await?;
        let total = messages.len();
        if total <= keep {
            return Ok(0);
        }
        let removed = total - keep;
        let trimmed: Vec<Message> = messages.into_iter().skip(removed).collect();
        self.save_transcript(session_id, &trimmed).await?;
        Ok(removed)
    }

    async fn usage(&self, session_id: &str) -> Result<SessionUsage> {
        let messages = self.load_transcript(session_id).await?;
        Ok(SessionUsage {
            message_count: messages.len(),
            estimated_tokens: estimate_tokens(&messages),
        })
    }

    async fn list_sessions(&self) -> Result<Vec<String>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.root)
            .await
            .map_err(|e| ClawzError::Internal(format!("list sessions: {e}")))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ClawzError::Internal(format!("list sessions: {e}")))?
        {
            if !entry
                .file_type()
                .await
                .map_err(|e| ClawzError::Internal(format!("list sessions: {e}")))?
                .is_dir()
            {
                continue;
            }
            let path = entry.path().join("transcript.jsonl");
            if path.exists() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::message::Message;

    #[tokio::test]
    async fn file_session_roundtrip() {
        let dir = std::env::temp_dir().join(format!("clawz-sess-{}", uuid::Uuid::new_v4()));
        let store = FileSessionStore::new(&dir);
        let id = "test-session";

        store
            .append_message(id, &Message::user("hello"))
            .await
            .unwrap();
        store
            .append_message(id, &Message::assistant("hi"))
            .await
            .unwrap();

        let loaded = store.load_transcript(id).await.unwrap();
        assert_eq!(loaded.len(), 2);

        let removed = store.compact(id, 1).await.unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.load_transcript(id).await.unwrap().len(), 1);

        store.reset(id).await.unwrap();
        assert!(store.load_transcript(id).await.unwrap().is_empty());
    }
}
