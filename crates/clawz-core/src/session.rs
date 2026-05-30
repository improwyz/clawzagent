//! Durable agent session transcripts and session keys.
//!
//! Used by the worker to persist conversation history across turns (REST,
//! channels, CLI). Enterprise deployments can add a Postgres backend later;
//! standalone mode uses the worker's filesystem store.

use async_trait::async_trait;

use crate::error::Result;
use crate::types::message::Message;

/// Stable identity for a conversation thread across channels.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionKey {
    pub tenant_id: String,
    pub channel: String,
    pub peer_id: String,
    pub agent_id: String,
}

impl SessionKey {
    pub fn new(
        tenant_id: impl Into<String>,
        channel: impl Into<String>,
        peer_id: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            channel: channel.into(),
            peer_id: peer_id.into(),
            agent_id: agent_id.into(),
        }
    }

    /// REST/API sessions keyed by client-provided conversation id.
    pub fn from_api(agent_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self::new("default", "api", conversation_id, agent_id)
    }

    /// Channel webhooks (Slack, SMS, etc.) — stable id per peer on a channel.
    pub fn from_channel(
        channel_type: impl Into<String>,
        channel_id: impl Into<String>,
        peer_id: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        let channel_id = channel_id.into();
        let peer_id = peer_id.into();
        Self::new(
            "default",
            channel_type,
            format!("{channel_id}:{peer_id}"),
            agent_id,
        )
    }

    /// Filesystem-safe storage path segment (no slashes in components).
    pub fn storage_id(&self) -> String {
        fn seg(s: &str) -> String {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                .collect()
        }
        format!(
            "{}__{}__{}__{}",
            seg(&self.tenant_id),
            seg(&self.channel),
            seg(&self.peer_id),
            seg(&self.agent_id),
        )
    }
}

/// Lightweight usage stats for `/usage` and dashboards.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SessionUsage {
    pub message_count: usize,
    pub estimated_tokens: usize,
}

/// Append-only transcript storage with reset and compact.
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn load_transcript(&self, session_id: &str) -> Result<Vec<Message>>;

    async fn save_transcript(&self, session_id: &str, messages: &[Message]) -> Result<()>;

    async fn append_message(&self, session_id: &str, message: &Message) -> Result<()> {
        let mut messages = self.load_transcript(session_id).await?;
        messages.push(message.clone());
        self.save_transcript(session_id, &messages).await
    }

    async fn reset(&self, session_id: &str) -> Result<()>;

    /// Keep the last `keep_last` messages; returns number of messages removed.
    async fn compact(&self, session_id: &str, keep_last: usize) -> Result<usize>;

    async fn usage(&self, session_id: &str) -> Result<SessionUsage>;

    /// List known session ids (filesystem store scans directories; default empty).
    async fn list_sessions(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}
