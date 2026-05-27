//! Provider adapter subsystem.
//!
//! This module defines the [`ProviderAdapter`] trait and a set of concrete
//! adapters that map ClawZ's internal [`ChatRequest`] / [`ChatResponse`] types
//! to each provider's native HTTP API.
//!
//! ## Shared wire-format types
//!
//! Most adapters speak a dialect of the OpenAI wire format.  The [`WireMessage`],
//! [`WireContent`], and [`WireToolCall`] types are reused across adapters to
//! avoid duplicating serialization logic.
//!
//! // Dependency: `clawz_core::types::message::*` and `clawz_core::types::tool::ToolSchema`
//! // for the canonical request / response / tool schemas.

pub mod anthropic;
pub mod azure;
pub mod bedrock;
pub mod deepseek;
pub mod gemini;
pub mod ollama;
pub mod openai;
pub mod stub;

use std::pin::Pin;

use async_trait::async_trait;
use clawz_core::{
    error::ClawzError,
    types::{message::*, tool::ToolSchema},
};
use futures_core::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Wire format types shared across OpenAI-compatible adapters ────────────────

/// OpenAI-wire-format message (used internally between router and adapters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    pub role: String,
    /// Content may be a plain string or an array of content blocks.
    pub content: WireContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Content payload for a [`WireMessage`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[derive(Default)]
pub enum WireContent {
    Text(String),
    Parts(Vec<WireContentPart>),
    #[default]
    Null,
}

impl WireContent {
    /// Return the plain-text representation, if any.
    pub fn as_text(&self) -> &str {
        match self {
            WireContent::Text(t) => t.as_str(),
            WireContent::Parts(parts) => parts
                .iter()
                .find_map(|p| {
                    if let WireContentPart::Text { text } = p {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or(""),
            WireContent::Null => "",
        }
    }
}


/// A single part of a multimodal message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireContentPart {
    Text { text: String },
    ImageUrl { image_url: WireImageUrl },
}

/// Image URL descriptor for multimodal content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireImageUrl {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Tool invocation embedded in an assistant message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: WireFunction,
}

/// Function payload inside a [`WireToolCall`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireFunction {
    pub name: String,
    pub arguments: String,
}

// ── ProviderAdapter trait ─────────────────────────────────────────────────────

/// Core trait that every provider adapter must implement.
///
/// Adapters are responsible for:
/// - Translating [`ChatRequest`] into provider-specific JSON.
/// - Parsing provider-specific responses into [`ChatResponse`].
/// - Handling streaming via [`futures_core::Stream`].
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// Human-readable provider identifier (e.g. "openai").
    fn provider_name(&self) -> &str;

    /// Human-readable list of models this adapter handles.
    fn supported_models(&self) -> Vec<&'static str>;

    /// Perform a non-streaming chat completion.
    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError>;

    /// Perform a streaming chat completion.
    async fn chat_stream(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ClawzError>> + Send>>, ClawzError>;

    /// Return the context-window size (in tokens) for a given model.
    fn context_window(&self, model: &str) -> u32;

    /// Estimate USD cost for a hypothetical call.
    fn estimate_cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64;
}

// ── AdapterConfig ─────────────────────────────────────────────────────────────

/// Resolved runtime config for a single adapter invocation.
///
/// Created by the router from [`ProviderConfig`] before each call.
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    /// Final URL endpoint (may include deployment paths for Azure).
    pub endpoint: String,
    /// Resolved API key (env vars already expanded).
    pub api_key: String,
    /// Extra headers (e.g. Azure api-version).
    pub extra_headers: Vec<(String, String)>,
    /// Extra query parameters.
    pub extra_query: Vec<(String, String)>,
    /// Provider-specific extras (region, deployment, etc.)
    pub extras: std::collections::HashMap<String, String>,
}

// ── Shared conversion helpers ─────────────────────────────────────────────────

/// Convert clawz-core [`Message`] to [`WireMessage`] (OpenAI format).
///
/// Handles text, tool calls, tool results, and multimodal content.
pub fn to_wire_message(msg: &Message) -> WireMessage {
    match &msg.content {
        MessageContent::Text(text) => WireMessage {
            role: msg.role.as_str().to_string(),
            content: WireContent::Text(text.clone()),
            name: msg.name.clone(),
            tool_calls: None,
            tool_call_id: None,
        },
        MessageContent::ToolCalls(calls) => {
            let wire_calls: Vec<WireToolCall> = calls
                .iter()
                .map(|tc| WireToolCall {
                    id: tc.id.clone(),
                    call_type: "function".to_string(),
                    function: WireFunction {
                        name: tc.name.clone(),
                        arguments: tc.arguments.to_string(),
                    },
                })
                .collect();
            WireMessage {
                role: "assistant".to_string(),
                content: WireContent::Null,
                name: msg.name.clone(),
                tool_calls: Some(wire_calls),
                tool_call_id: None,
            }
        }
        MessageContent::ToolResult(result) => WireMessage {
            role: "tool".to_string(),
            content: WireContent::Text(result.output.clone()),
            name: None,
            tool_calls: None,
            tool_call_id: Some(result.tool_call_id.clone()),
        },
        MessageContent::Multimodal(parts) => {
            let wire_parts: Vec<WireContentPart> = parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(WireContentPart::Text { text: text.clone() }),
                    ContentPart::ImageUrl { url, detail } => Some(WireContentPart::ImageUrl {
                        image_url: WireImageUrl {
                            url: url.clone(),
                            detail: detail.clone(),
                        },
                    }),
                    ContentPart::ImageBase64 { media_type, data } => {
                        Some(WireContentPart::ImageUrl {
                            image_url: WireImageUrl {
                                url: format!("data:{};base64,{}", media_type, data),
                                detail: None,
                            },
                        })
                    }
                    ContentPart::AudioBase64 { .. } => None, // unsupported in wire format
                })
                .collect();
            WireMessage {
                role: msg.role.as_str().to_string(),
                content: WireContent::Parts(wire_parts),
                name: msg.name.clone(),
                tool_calls: None,
                tool_call_id: None,
            }
        }
    }
}

/// Build OpenAI-format tools array from [`ToolSchema`] vec.
pub fn to_wire_tools(tools: &[ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect()
}

/// Parse an HTTP status into a [`ClawzError`], consuming the body for context.
///
/// Recognises 429 (rate limited), 401/403 (auth), and generic provider errors.
pub async fn http_error(response: reqwest::Response) -> ClawzError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status.as_u16() == 429 {
        // Try to parse retry-after from body
        let retry_secs = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| {
                v["error"]["retry_after"].as_u64()
                    .or_else(|| v["retry_after"].as_u64())
            })
            .unwrap_or(60);
        ClawzError::RateLimited { retry_after_secs: retry_secs }
    } else if status.as_u16() == 401 || status.as_u16() == 403 {
        ClawzError::Auth(format!("HTTP {}: {}", status, body))
    } else {
        ClawzError::Provider(format!("HTTP {}: {}", status, body))
    }
}
