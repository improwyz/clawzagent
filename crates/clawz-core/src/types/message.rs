//! Message types for LLM chat — roles, content, requests, responses, and streaming.
//!
//! Designed to be compatible with OpenAI's Chat Completions API so that
//! provider adapters can map to/from provider-native formats with minimal
//! boilerplate.
//!
//! // Dependency: used by worker::provider adapters, worker::pipeline, gateway::chat_handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::tool::{ToolCall, ToolResult};

// ── Role ─────────────────────────────────────────────────────────────────────

/// Message author in a conversation turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        }
    }
}

// ── Content parts (multimodal) ────────────────────────────────────────────────

/// A single piece of multimodal content (text, image, audio).
/// // Dependency: used by MessageContent::Multimodal variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { url: String, detail: Option<String> },
    ImageBase64 { media_type: String, data: String },
    AudioBase64 { media_type: String, data: String },
}

// ── Message content variants ──────────────────────────────────────────────────

/// The payload of a `Message` — text, tool calls, tool results, or mixed media.
/// // Dependency: serialized into JSONB in db::DbMessage.content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain text content.
    Text(String),
    /// One or more tool calls issued by the assistant.
    ToolCalls(Vec<ToolCall>),
    /// The result returned after executing a tool.
    ToolResult(ToolResult),
    /// Mixed/multimodal content (text + images + audio).
    Multimodal(Vec<ContentPart>),
}

impl MessageContent {
    /// Convenience constructor for plain text.
    pub fn text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }

    /// Extract the text content if this is a `Text` variant.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(t) => Some(t.as_str()),
            _ => None,
        }
    }
}

// ── Message ───────────────────────────────────────────────────────────────────

/// A single turn in a conversation.
/// // Dependency: stored by db::MessageRepo, passed through worker::pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub role: Role,
    pub content: MessageContent,
    pub created_at: DateTime<Utc>,
    /// Optional name field (OpenAI function-calling style).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Default for Message {
    fn default() -> Self {
        Self::new(Role::User, MessageContent::text(""))
    }
}

impl Message {
    pub fn new(role: Role, content: MessageContent) -> Self {
        Self {
            id: Uuid::new_v4(),
            role,
            content,
            created_at: Utc::now(),
            name: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, MessageContent::text(text))
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, MessageContent::text(text))
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(Role::System, MessageContent::text(text))
    }
}

// ── Chat request / response ───────────────────────────────────────────────────

/// Request payload sent to a provider adapter.
/// // Dependency: passed to traits::Provider::chat and chat_stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    /// Optional system prompt override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Tools available for this request (empty = no tool calling).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<crate::types::tool::ToolSchema>,
    /// Whether to stream the response.
    #[serde(default)]
    pub stream: bool,
    /// Stop sequences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// End-user identifier (for provider abuse tracking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl Default for ChatRequest {
    fn default() -> Self {
        Self::new("", vec![])
    }
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: false,
            stop: None,
            user: None,
        }
    }
}

/// Provider response — one or more choices plus token accounting.
/// // Dependency: returned by traits::Provider, consumed by worker::pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Usage,
    /// OpenAI-style object type string (e.g. "chat.completion").
    #[serde(default = "default_object")]
    pub object: String,
    /// Unix timestamp seconds (for OpenAI-compatible APIs).
    #[serde(default)]
    pub created: u64,
    /// Wall-clock time this response was received.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

fn default_object() -> String {
    "chat.completion".to_string()
}

impl Default for ChatResponse {
    fn default() -> Self {
        Self {
            id: String::new(),
            model: String::new(),
            choices: Vec::new(),
            usage: Usage::default(),
            object: default_object(),
            created: 0,
            created_at: Utc::now(),
        }
    }
}

impl ChatResponse {
    /// Return the text content of the first choice, if any.
    pub fn first_text(&self) -> Option<&str> {
        self.choices.first()?.message.content.as_text()
    }
}

/// A single choice (completion) inside a `ChatResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: usize,
    pub message: Message,
    /// Raw finish reason string as returned by the provider.
    pub finish_reason: Option<String>,
}

/// Why the model stopped generating tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
}

// ── Usage / token accounting ──────────────────────────────────────────────────

/// Token counts for a single request/response.
/// // Dependency: accumulated into metrics and cost tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

impl Usage {
    pub fn new(prompt: usize, completion: usize) -> Self {
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
        }
    }

    pub fn add(&mut self, other: &Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }
}

// ── Streaming ─────────────────────────────────────────────────────────────────

/// A single delta in a server-sent event stream.
/// // Dependency: produced by traits::Provider::chat_stream, consumed by gateway SSE endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub id: String,
    pub model: String,
    /// Index of the choice this delta belongs to.
    pub choice_index: u32,
    /// Incremental text delta (may be empty for first/last chunks).
    pub delta_text: Option<String>,
    /// Incremental tool-call delta.
    pub delta_tool_call: Option<PartialToolCall>,
    pub finish_reason: Option<FinishReason>,
    /// Cumulative usage (only populated on the final chunk by most providers).
    pub usage: Option<Usage>,
}

/// Partial tool-call data accumulated across stream chunks.
/// // Dependency: reassembled by worker::pipeline into a complete ToolCall.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialToolCall {
    pub index: u32,
    pub id: Option<String>,
    pub name: Option<String>,
    /// Raw JSON fragment — callers must accumulate and parse when complete.
    pub arguments_fragment: Option<String>,
}
