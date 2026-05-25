//! Channel (messaging platform) types — capabilities, config, messages.
//!
//! Channel plugins bridge ClawZ agents to external messaging platforms
//! (Slack, Discord, Telegram, WhatsApp, email, …).
//!
//! // Dependency: used by worker::channel plugins, gateway::webhook_handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ── ChannelCapabilities ───────────────────────────────────────────────────────

/// Feature flags advertised by a channel plugin.
/// // Dependency: returned by traits::ChannelPlugin::capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelCapabilities {
    pub media: bool,
    pub typing: bool,
    pub reactions: bool,
    pub threads: bool,
    pub voice: bool,
    pub video: bool,
    pub file_upload: bool,
}

impl ChannelCapabilities {
    pub fn text_only() -> Self {
        Self::default()
    }

    pub fn full() -> Self {
        Self {
            media: true,
            typing: true,
            reactions: true,
            threads: true,
            voice: true,
            video: true,
            file_upload: true,
        }
    }

    pub fn discord() -> Self {
        Self {
            media: true,
            typing: true,
            reactions: true,
            threads: true,
            voice: true,
            video: false,
            file_upload: true,
        }
    }

    pub fn slack() -> Self {
        Self {
            media: true,
            typing: true,
            reactions: true,
            threads: true,
            voice: false,
            video: false,
            file_upload: true,
        }
    }
}

// ── ChannelConfig ─────────────────────────────────────────────────────────────

/// Per-channel configuration — platform, credentials, webhook URL.
/// // Dependency: stored in db, passed to traits::ChannelPlugin via ChannelContext.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub id: Uuid,
    /// e.g. "slack", "discord", "telegram", "whatsapp", "email"
    pub platform: String,
    /// Platform-specific credentials (token, client_id, etc.)
    pub credentials: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    pub enabled: bool,
    pub capabilities: ChannelCapabilities,
}

impl ChannelConfig {
    pub fn new(platform: impl Into<String>, credentials: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            platform: platform.into(),
            credentials,
            webhook_url: None,
            enabled: true,
            capabilities: ChannelCapabilities::default(),
        }
    }

    pub fn with_webhook(mut self, url: impl Into<String>) -> Self {
        self.webhook_url = Some(url.into());
        self
    }

    pub fn with_capabilities(mut self, caps: ChannelCapabilities) -> Self {
        self.capabilities = caps;
        self
    }
}

// ── Attachment ────────────────────────────────────────────────────────────────

/// File attached to a message.
/// // Dependency: embedded in IncomingMessage and OutgoingMessage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    pub size_bytes: u64,
}

// ── IncomingMessage ───────────────────────────────────────────────────────────

/// A message received from a channel (user → agent).
/// // Dependency: produced by traits::ChannelPlugin::receive, consumed by worker::pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

impl IncomingMessage {
    pub fn new(
        channel_id: Uuid,
        sender_id: impl Into<String>,
        sender_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            channel_id,
            sender_id: sender_id.into(),
            sender_name: sender_name.into(),
            content: content.into(),
            attachments: Vec::new(),
            thread_id: None,
            timestamp: Utc::now(),
            metadata: Default::default(),
        }
    }
}

// ── OutgoingMessage ───────────────────────────────────────────────────────────

/// A message sent to a channel (agent → user).
/// // Dependency: produced by worker::pipeline, sent via traits::ChannelPlugin::send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub channel_id: Uuid,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

impl OutgoingMessage {
    pub fn new(channel_id: Uuid, content: impl Into<String>) -> Self {
        Self {
            channel_id,
            content: content.into(),
            attachments: Vec::new(),
            reply_to: None,
            metadata: Default::default(),
        }
    }

    pub fn reply(mut self, message_id: impl Into<String>) -> Self {
        self.reply_to = Some(message_id.into());
        self
    }
}
