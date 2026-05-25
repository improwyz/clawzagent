//! OpenAI adapter — native `/v1/chat/completions` endpoint.
//!
//! Supports all GPT-4, GPT-4o, o1, and o3 family models plus streaming via
//! Server-Sent Events (SSE).
//!
//! // Dependency: `super::{http_error, to_wire_message, to_wire_tools, AdapterConfig, ProviderAdapter}`

use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use clawz_core::{
    error::ClawzError,
    types::{
        message::*,
        tool::ToolCall,
    },
};
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    http_error, to_wire_message, to_wire_tools, AdapterConfig, ProviderAdapter,
    WireToolCall,
};

// ── Request body ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: Vec<super::WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<&'a Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<&'a str>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    id: String,
    model: String,
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
    created: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    index: usize,
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    role: Option<String>,
    content: Option<Value>,
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

// ── Stream chunk types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    id: String,
    model: String,
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    index: u32,
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    index: u32,
    id: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

// ── Adapter ───────────────────────────────────────────────────────────────────

/// Adapter for the OpenAI API and OpenAI-compatible endpoints.
pub struct OpenAiAdapter;

#[async_trait]
impl ProviderAdapter for OpenAiAdapter {
    fn provider_name(&self) -> &str {
        "openai"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec![
            "gpt-4",
            "gpt-4-turbo",
            "gpt-4-turbo-preview",
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4o-audio-preview",
            "gpt-3.5-turbo",
            "gpt-3.5-turbo-16k",
            "o1",
            "o1-mini",
            "o1-preview",
            "o3",
            "o3-mini",
        ]
    }

    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        let body = build_request(request, false);
        let url = format!("{}/chat/completions", config.endpoint.trim_end_matches('/'));

        let mut req = client
            .post(&url)
            .bearer_auth(&config.api_key)
            .json(&body);

        for (k, v) in &config.extra_headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let resp = req.send().await.map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let raw: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| ClawzError::Provider(format!("parse error: {e}")))?;

        Ok(parse_response(raw))
    }

    async fn chat_stream(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ClawzError>> + Send>>, ClawzError>
    {
        let body = build_request(request, true);
        let url = format!("{}/chat/completions", config.endpoint.trim_end_matches('/'));

        let mut req = client
            .post(&url)
            .bearer_auth(&config.api_key)
            .json(&body);

        for (k, v) in &config.extra_headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let resp = req.send().await.map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let byte_stream = resp.bytes_stream();
        let stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(stream))
    }

    fn context_window(&self, model: &str) -> u32 {
        match model {
            m if m.starts_with("gpt-4o") => 128_000,
            m if m.starts_with("gpt-4-turbo") => 128_000,
            "gpt-4" => 8_192,
            "gpt-3.5-turbo-16k" => 16_385,
            "gpt-3.5-turbo" => 16_385,
            m if m.starts_with("o1") || m.starts_with("o3") => 200_000,
            _ => 128_000,
        }
    }

    fn estimate_cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        let (in_per_1k, out_per_1k) = match model {
            "gpt-4o" => (0.005, 0.015),
            "gpt-4o-mini" => (0.00015, 0.0006),
            m if m.starts_with("gpt-4-turbo") => (0.01, 0.03),
            "gpt-4" => (0.03, 0.06),
            m if m.starts_with("gpt-3.5-turbo") => (0.0005, 0.0015),
            m if m.starts_with("o1") => (0.015, 0.060),
            m if m.starts_with("o3") => (0.010, 0.040),
            _ => (0.005, 0.015),
        };
        (input_tokens as f64 / 1_000.0) * in_per_1k
            + (output_tokens as f64 / 1_000.0) * out_per_1k
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the JSON body for an OpenAI chat completion request.
fn build_request(request: &ChatRequest, stream: bool) -> Value {
    let messages: Vec<super::WireMessage> = request.messages.iter().map(to_wire_message).collect();
    let tools = to_wire_tools(&request.tools);

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "stream": stream,
    });

    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    if let Some(max) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max);
    }
    if let Some(stop) = &request.stop {
        body["stop"] = serde_json::json!(stop);
    }
    if let Some(user) = &request.user {
        body["user"] = serde_json::json!(user);
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::json!(tools);
    }
    if stream {
        body["stream_options"] = serde_json::json!({"include_usage": true});
    }

    body
}

/// Convert the raw OpenAI response into our canonical [`ChatResponse`].
fn parse_response(raw: OpenAiResponse) -> ChatResponse {
    let choices: Vec<ChatChoice> = raw
        .choices
        .into_iter()
        .map(|c| {
            let content = parse_openai_message(c.message);
            ChatChoice {
                index: c.index,
                message: Message {
                    id: Uuid::new_v4(),
                    role: Role::Assistant,
                    content,
                    created_at: Utc::now(),
                    name: None,
                },
                finish_reason: c.finish_reason,
            }
        })
        .collect();

    let usage = raw.usage.map(|u| Usage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
    }).unwrap_or_default();

    ChatResponse {
        id: raw.id,
        model: raw.model,
        choices,
        usage,
        object: "chat.completion".to_string(),
        created: raw.created.unwrap_or(0),
        created_at: Utc::now(),
    }
}

/// Extract the canonical [`MessageContent`] from an OpenAI message payload.
fn parse_openai_message(msg: OpenAiMessage) -> MessageContent {
    // Tool calls take priority
    if let Some(tool_calls) = msg.tool_calls {
        if !tool_calls.is_empty() {
            let calls: Vec<ToolCall> = tool_calls
                .into_iter()
                .map(|tc| ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments: serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(Value::String(tc.function.arguments.clone())),
                })
                .collect();
            return MessageContent::ToolCalls(calls);
        }
    }

    match msg.content {
        Some(Value::String(text)) => MessageContent::Text(text),
        Some(Value::Array(parts)) => {
            // Parse multimodal content
            let text = parts
                .iter()
                .filter_map(|p| {
                    if p["type"] == "text" {
                        p["text"].as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            MessageContent::Text(text)
        }
        _ => MessageContent::Text(String::new()),
    }
}

/// Parse an SSE byte stream into [`StreamChunk`]s.
fn parse_sse_stream(
    byte_stream: impl futures_core::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl Stream<Item = Result<StreamChunk, ClawzError>> + Send {
    let mut buf = String::new();

    async_stream::stream! {
        futures_util::pin_mut!(byte_stream);
        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    yield Err(ClawzError::Transport(e.to_string()));
                    return;
                }
            };
            let text = match std::str::from_utf8(&chunk) {
                Ok(s) => s,
                Err(e) => {
                    yield Err(ClawzError::Provider(format!("UTF-8 error: {e}")));
                    return;
                }
            };
            buf.push_str(text);

            // Process complete SSE lines
            while let Some(newline_pos) = buf.find('\n') {
                let line = buf[..newline_pos].trim_end_matches('\r').to_string();
                buf = buf[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        return;
                    }
                    match serde_json::from_str::<OpenAiStreamChunk>(data) {
                        Ok(chunk) => {
                            for choice in chunk.choices {
                                let delta_text = choice.delta.content.clone();
                                let delta_tool_call = choice
                                    .delta
                                    .tool_calls
                                    .as_ref()
                                    .and_then(|tcs| tcs.first())
                                    .map(|tc| PartialToolCall {
                                        index: tc.index,
                                        id: tc.id.clone(),
                                        name: tc.function.as_ref().and_then(|f| f.name.clone()),
                                        arguments_fragment: tc
                                            .function
                                            .as_ref()
                                            .and_then(|f| f.arguments.clone()),
                                    });

                                let finish_reason = choice.finish_reason.as_deref().map(parse_finish_reason);

                                let usage = if choice.finish_reason.is_some() {
                                    chunk.usage.as_ref().map(|u| Usage {
                                        prompt_tokens: u.prompt_tokens,
                                        completion_tokens: u.completion_tokens,
                                        total_tokens: u.total_tokens,
                                    })
                                } else {
                                    None
                                };

                                if delta_text.is_some()
                                    || delta_tool_call.is_some()
                                    || finish_reason.is_some()
                                {
                                    yield Ok(StreamChunk {
                                        id: chunk.id.clone(),
                                        model: chunk.model.clone(),
                                        choice_index: choice.index,
                                        delta_text,
                                        delta_tool_call,
                                        finish_reason,
                                        usage,
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            yield Err(ClawzError::Provider(format!(
                                "SSE parse error: {e} — data: {data}"
                            )));
                        }
                    }
                }
            }
        }
    }
}

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_window() {
        let adapter = OpenAiAdapter;
        assert_eq!(adapter.context_window("gpt-4o"), 128_000);
        assert_eq!(adapter.context_window("gpt-4"), 8_192);
        assert_eq!(adapter.context_window("o1"), 200_000);
    }

    #[test]
    fn test_estimate_cost() {
        let adapter = OpenAiAdapter;
        let cost = adapter.estimate_cost_usd("gpt-4o", 1_000, 500);
        // 1000 input at $0.005/1k = $0.005, 500 output at $0.015/1k = $0.0075
        assert!((cost - 0.0125).abs() < 0.0001);
    }

    #[test]
    fn test_build_request_includes_tools() {
        let request = ChatRequest {
            model: "gpt-4o".to_string(),
            messages: vec![Message::user("hi")],
            tools: vec![clawz_core::types::tool::ToolSchema::no_args(
                "test_tool",
                "A test tool",
            )],
            ..Default::default()
        };
        let body = build_request(&request, false);
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_parse_openai_message_text() {
        let msg = OpenAiMessage {
            role: Some("assistant".to_string()),
            content: Some(Value::String("Hello!".to_string())),
            tool_calls: None,
        };
        let content = parse_openai_message(msg);
        assert!(matches!(content, MessageContent::Text(t) if t == "Hello!"));
    }

    #[test]
    fn test_parse_openai_message_tool_calls() {
        let msg = OpenAiMessage {
            role: Some("assistant".to_string()),
            content: None,
            tool_calls: Some(vec![WireToolCall {
                id: "call_abc".to_string(),
                call_type: "function".to_string(),
                function: super::super::WireFunction {
                    name: "get_weather".to_string(),
                    arguments: r#"{"location":"SF"}"#.to_string(),
                },
            }]),
        };
        let content = parse_openai_message(msg);
        assert!(matches!(content, MessageContent::ToolCalls(calls) if calls.len() == 1));
    }

    #[test]
    fn test_normalize_response() {
        let raw = OpenAiResponse {
            id: "chatcmpl-test".to_string(),
            model: "gpt-4o".to_string(),
            choices: vec![OpenAiChoice {
                index: 0,
                message: OpenAiMessage {
                    role: Some("assistant".to_string()),
                    content: Some(Value::String("Hi there!".to_string())),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(OpenAiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
            created: Some(1_700_000_000),
        };

        let response = parse_response(raw);
        assert_eq!(response.id, "chatcmpl-test");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.usage.total_tokens, 15);
        assert_eq!(response.first_text(), Some("Hi there!"));
    }
}
