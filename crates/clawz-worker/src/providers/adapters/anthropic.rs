use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use clawz_core::{
    error::ClawzError,
    types::{
        message::*,
        tool::{ToolCall, ToolSchema},
    },
};
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{http_error, AdapterConfig, ProviderAdapter};

const ANTHROPIC_VERSION: &str = "2023-06-01";

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicBlock>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicBlock {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<AnthropicBlock>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: usize,
    output_tokens: usize,
}

// ── Streaming event types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamEvent {
    MessageStart {
        message: AnthropicStreamMessage,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicStreamBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        #[allow(dead_code)]
        index: u32,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: Option<AnthropicUsage>,
    },
    MessageStop,
    Error {
        error: AnthropicStreamError,
    },
    Ping,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessage {
    id: String,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamBlock {
    Text {
        #[allow(dead_code)]
        text: String,
    },
    ToolUse { id: String, name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

// ── Adapter ───────────────────────────────────────────────────────────────────

pub struct AnthropicAdapter;

#[async_trait]
impl ProviderAdapter for AnthropicAdapter {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec![
            "claude-opus-4-5",
            "claude-sonnet-4-5",
            "claude-haiku-3",
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
            "claude-3-opus-20240229",
            "claude-3-sonnet-20240229",
            "claude-3-haiku-20240307",
        ]
    }

    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        let body = build_request(request, false)?;
        let url = format!("{}/messages", config.endpoint.trim_end_matches('/'));

        let resp = client
            .post(&url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let raw: AnthropicResponse = resp
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
        let body = build_request(request, true)?;
        let url = format!("{}/messages", config.endpoint.trim_end_matches('/'));

        let resp = client
            .post(&url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let byte_stream = resp.bytes_stream();
        let stream = parse_sse_stream(byte_stream);
        Ok(Box::pin(stream))
    }

    fn context_window(&self, model: &str) -> u32 {
        match model {
            m if m.contains("opus") => 200_000,
            m if m.contains("sonnet") => 200_000,
            m if m.contains("haiku") => 200_000,
            _ => 200_000,
        }
    }

    fn estimate_cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        let (in_per_1k, out_per_1k) = match model {
            m if m.contains("opus") => (0.015, 0.075),
            m if m.contains("sonnet") => (0.003, 0.015),
            m if m.contains("haiku") => (0.00025, 0.00125),
            _ => (0.003, 0.015),
        };
        (input_tokens as f64 / 1_000.0) * in_per_1k
            + (output_tokens as f64 / 1_000.0) * out_per_1k
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_request(request: &ChatRequest, stream: bool) -> Result<AnthropicRequest, ClawzError> {
    // Extract system prompt (from ChatRequest.system_prompt OR from System messages)
    let mut system: Option<String> = request.system_prompt.clone();
    let mut user_messages: Vec<&Message> = Vec::new();

    for msg in &request.messages {
        if msg.role == Role::System {
            let text = match &msg.content {
                MessageContent::Text(t) => t.clone(),
                _ => serde_json::to_string(&msg.content)
                    .map_err(|e| ClawzError::Serialization(e.to_string()))?,
            };
            match system {
                None => system = Some(text),
                Some(ref mut s) => {
                    s.push('\n');
                    s.push_str(&text);
                }
            }
        } else {
            user_messages.push(msg);
        }
    }

    let messages: Vec<AnthropicMessage> = user_messages
        .iter()
        .map(|msg| convert_message(msg))
        .collect::<Result<_, _>>()?;

    let tools = build_tools(&request.tools);

    Ok(AnthropicRequest {
        model: request.model.clone(),
        messages,
        max_tokens: request.max_tokens.unwrap_or(4_096),
        system,
        temperature: request.temperature,
        stop_sequences: request.stop.clone(),
        tools,
        stream,
    })
}

fn convert_message(msg: &Message) -> Result<AnthropicMessage, ClawzError> {
    let role = match msg.role {
        Role::User | Role::Tool => "user".to_string(),
        Role::Assistant => "assistant".to_string(),
        Role::System => "user".to_string(), // system already stripped out
    };

    let content = match &msg.content {
        MessageContent::Text(text) => AnthropicContent::Text(text.clone()),
        MessageContent::ToolCalls(calls) => {
            let blocks: Vec<AnthropicBlock> = calls
                .iter()
                .map(|tc| AnthropicBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                })
                .collect();
            AnthropicContent::Blocks(blocks)
        }
        MessageContent::ToolResult(result) => {
            AnthropicContent::Blocks(vec![AnthropicBlock::ToolResult {
                tool_use_id: result.tool_call_id.clone(),
                content: result.output.clone(),
                is_error: result.is_error,
            }])
        }
        MessageContent::Multimodal(parts) => {
            let blocks: Vec<AnthropicBlock> = parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => {
                        Some(AnthropicBlock::Text { text: text.clone() })
                    }
                    ContentPart::ImageBase64 { media_type, data } => {
                        Some(AnthropicBlock::Image {
                            source: AnthropicImageSource {
                                source_type: "base64".to_string(),
                                media_type: media_type.clone(),
                                data: data.clone(),
                            },
                        })
                    }
                    ContentPart::ImageUrl { url, .. } => {
                        // Anthropic doesn't support URL images directly; skip or use text
                        Some(AnthropicBlock::Text { text: format!("[image: {}]", url) })
                    }
                    ContentPart::AudioBase64 { .. } => None,
                })
                .collect();
            AnthropicContent::Blocks(blocks)
        }
    };

    Ok(AnthropicMessage { role, content })
}

fn build_tools(schemas: &[ToolSchema]) -> Vec<AnthropicTool> {
    schemas
        .iter()
        .map(|s| AnthropicTool {
            name: s.name.clone(),
            description: s.description.clone(),
            input_schema: s.parameters.clone(),
        })
        .collect()
}

fn parse_response(raw: AnthropicResponse) -> ChatResponse {
    let message_content = parse_content_blocks(raw.content);

    ChatResponse {
        id: raw.id,
        model: raw.model,
        choices: vec![ChatChoice {
            index: 0,
            message: Message {
                id: Uuid::new_v4(),
                role: Role::Assistant,
                content: message_content,
                created_at: Utc::now(),
                name: None,
            },
            finish_reason: raw.stop_reason,
        }],
        usage: Usage {
            prompt_tokens: raw.usage.input_tokens,
            completion_tokens: raw.usage.output_tokens,
            total_tokens: raw.usage.input_tokens + raw.usage.output_tokens,
        },
        object: "chat.completion".to_string(),
        created: 0,
        created_at: Utc::now(),
    }
}

fn parse_content_blocks(blocks: Vec<AnthropicBlock>) -> MessageContent {
    // If all blocks are tool_use, return ToolCalls
    let tool_calls: Vec<ToolCall> = blocks
        .iter()
        .filter_map(|b| {
            if let AnthropicBlock::ToolUse { id, name, input } = b {
                Some(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.clone(),
                })
            } else {
                None
            }
        })
        .collect();

    if !tool_calls.is_empty() {
        return MessageContent::ToolCalls(tool_calls);
    }

    // Otherwise, join text blocks
    let text = blocks
        .iter()
        .filter_map(|b| {
            if let AnthropicBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");

    MessageContent::Text(text)
}

fn parse_sse_stream(
    byte_stream: impl futures_core::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl Stream<Item = Result<StreamChunk, ClawzError>> + Send {
    let mut buf = String::new();
    let mut msg_id = String::new();
    let mut msg_model = String::new();
    let mut tool_name: Option<String> = None;
    let mut tool_id: Option<String> = None;
    let mut tool_index: u32 = 0;

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
                Ok(s) => s.to_string(),
                Err(e) => {
                    yield Err(ClawzError::Provider(format!("UTF-8 error: {e}")));
                    return;
                }
            };
            buf.push_str(&text);

            while let Some(newline_pos) = buf.find('\n') {
                let line = buf[..newline_pos].trim_end_matches('\r').to_string();
                buf = buf[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    match serde_json::from_str::<AnthropicStreamEvent>(data) {
                        Ok(event) => {
                            match event {
                                AnthropicStreamEvent::MessageStart { message } => {
                                    msg_id = message.id;
                                    msg_model = message.model;
                                }
                                AnthropicStreamEvent::ContentBlockStart { index, content_block } => {
                                    match content_block {
                                        AnthropicStreamBlock::ToolUse { id, name } => {
                                            tool_id = Some(id);
                                            tool_name = Some(name);
                                            tool_index = index;
                                        }
                                        AnthropicStreamBlock::Text { .. } => {}
                                    }
                                }
                                AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                                    match delta {
                                        AnthropicDelta::TextDelta { text } => {
                                            yield Ok(StreamChunk {
                                                id: msg_id.clone(),
                                                model: msg_model.clone(),
                                                choice_index: index,
                                                delta_text: Some(text),
                                                delta_tool_call: None,
                                                finish_reason: None,
                                                usage: None,
                                            });
                                        }
                                        AnthropicDelta::InputJsonDelta { partial_json } => {
                                            yield Ok(StreamChunk {
                                                id: msg_id.clone(),
                                                model: msg_model.clone(),
                                                choice_index: tool_index,
                                                delta_text: None,
                                                delta_tool_call: Some(PartialToolCall {
                                                    index: tool_index,
                                                    id: tool_id.clone(),
                                                    name: tool_name.clone(),
                                                    arguments_fragment: Some(partial_json),
                                                }),
                                                finish_reason: None,
                                                usage: None,
                                            });
                                            // After first chunk, don't re-send id/name
                                            tool_id = None;
                                            tool_name = None;
                                        }
                                    }
                                }
                                AnthropicStreamEvent::MessageDelta { delta, usage } => {
                                    let finish = delta.stop_reason.as_deref().map(|s| match s {
                                        "end_turn" => FinishReason::Stop,
                                        "max_tokens" => FinishReason::Length,
                                        "tool_use" => FinishReason::ToolCalls,
                                        _ => FinishReason::Stop,
                                    });
                                    let u = usage.map(|u| Usage {
                                        prompt_tokens: u.input_tokens,
                                        completion_tokens: u.output_tokens,
                                        total_tokens: u.input_tokens + u.output_tokens,
                                    });
                                    yield Ok(StreamChunk {
                                        id: msg_id.clone(),
                                        model: msg_model.clone(),
                                        choice_index: 0,
                                        delta_text: None,
                                        delta_tool_call: None,
                                        finish_reason: finish,
                                        usage: u,
                                    });
                                }
                                AnthropicStreamEvent::Error { error } => {
                                    yield Err(ClawzError::Provider(format!(
                                        "Anthropic stream error [{}]: {}",
                                        error.error_type, error.message
                                    )));
                                    return;
                                }
                                AnthropicStreamEvent::MessageStop
                                | AnthropicStreamEvent::ContentBlockStop { .. }
                                | AnthropicStreamEvent::Ping => {}
                            }
                        }
                        Err(e) => {
                            // Non-fatal parse errors on individual SSE events
                            log::warn!("Anthropic SSE parse error: {e} — data: {data}");
                        }
                    }
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_window() {
        let adapter = AnthropicAdapter;
        assert_eq!(adapter.context_window("claude-3-5-sonnet-20241022"), 200_000);
    }

    #[test]
    fn test_estimate_cost() {
        let adapter = AnthropicAdapter;
        // sonnet: $0.003/1k input, $0.015/1k output
        let cost = adapter.estimate_cost_usd("claude-3-5-sonnet-20241022", 1_000, 1_000);
        assert!((cost - 0.018).abs() < 0.0001);
    }

    #[test]
    fn test_build_request_separates_system() {
        let request = ChatRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![
                Message::system("You are helpful."),
                Message::user("Hello"),
            ],
            ..Default::default()
        };
        let body = build_request(&request, false).unwrap();
        assert_eq!(body.system, Some("You are helpful.".to_string()));
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].role, "user");
    }

    #[test]
    fn test_parse_response_text() {
        let raw = AnthropicResponse {
            id: "msg-test".to_string(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            content: vec![AnthropicBlock::Text {
                text: "Hello!".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };

        let response = parse_response(raw);
        assert_eq!(response.id, "msg-test");
        assert_eq!(response.usage.prompt_tokens, 10);
        assert_eq!(response.usage.completion_tokens, 5);
        assert_eq!(response.first_text(), Some("Hello!"));
    }

    #[test]
    fn test_parse_response_tool_use() {
        let raw = AnthropicResponse {
            id: "msg-tool".to_string(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            content: vec![AnthropicBlock::ToolUse {
                id: "toolu_01".to_string(),
                name: "get_weather".to_string(),
                input: serde_json::json!({"location": "SF"}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 10,
            },
        };

        let response = parse_response(raw);
        let choice = &response.choices[0];
        assert!(matches!(&choice.message.content, MessageContent::ToolCalls(calls) if calls.len() == 1));
    }

    #[test]
    fn test_convert_tool_result_message() {
        let msg = Message {
            id: Uuid::new_v4(),
            role: Role::Tool,
            content: MessageContent::ToolResult(clawz_core::types::tool::ToolResult::ok(
                "toolu_01",
                "sunny",
            )),
            created_at: Utc::now(),
            name: None,
        };
        let wire = convert_message(&msg).unwrap();
        assert_eq!(wire.role, "user");
        assert!(matches!(wire.content, AnthropicContent::Blocks(b) if b.len() == 1));
    }
}
