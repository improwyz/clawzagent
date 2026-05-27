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

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: GeminiInlineData },
    FunctionCall { function_call: GeminiFunctionCall },
    FunctionResponse { function_response: GeminiFunctionResponse },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GeminiInlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GeminiFunctionCall {
    name: String,
    args: Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GeminiFunctionResponse {
    name: String,
    response: Value,
}

#[derive(Debug, Serialize)]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct GeminiTool {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: Value,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
    model_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsageMetadata {
    prompt_token_count: Option<usize>,
    candidates_token_count: Option<usize>,
    total_token_count: Option<usize>,
}

// ── Adapter ───────────────────────────────────────────────────────────────────

pub struct GeminiAdapter;

#[async_trait]
impl ProviderAdapter for GeminiAdapter {
    fn provider_name(&self) -> &str {
        "gemini"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec![
            "gemini-1.5-pro",
            "gemini-1.5-flash",
            "gemini-1.5-flash-8b",
            "gemini-2.0-flash",
            "gemini-2.0-flash-exp",
            "gemini-pro",
        ]
    }

    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        let (body, model_path) = build_request(request)?;
        let api_key = &config.api_key;
        let base = config.endpoint.trim_end_matches('/');
        let url = format!("{base}/models/{model_path}:generateContent?key={api_key}");

        let resp = client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let raw: GeminiResponse = resp
            .json()
            .await
            .map_err(|e| ClawzError::Provider(format!("parse error: {e}")))?;

        Ok(parse_response(raw, &request.model))
    }

    async fn chat_stream(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ClawzError>> + Send>>, ClawzError>
    {
        let (body, model_path) = build_request(request)?;
        let api_key = &config.api_key;
        let base = config.endpoint.trim_end_matches('/');
        let url = format!("{base}/models/{model_path}:streamGenerateContent?alt=sse&key={api_key}");

        let resp = client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let model = request.model.clone();
        let byte_stream = resp.bytes_stream();
        let stream = parse_sse_stream(byte_stream, model);
        Ok(Box::pin(stream))
    }

    fn context_window(&self, model: &str) -> u32 {
        match model {
            m if m.contains("1.5-pro") => 1_000_000,
            m if m.contains("1.5-flash") => 1_000_000,
            m if m.contains("2.0") => 1_000_000,
            _ => 32_768,
        }
    }

    fn estimate_cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        let (in_per_1k, out_per_1k) = match model {
            m if m.contains("1.5-pro") => (0.00125, 0.005),
            m if m.contains("1.5-flash") => (0.000075, 0.0003),
            m if m.contains("2.0-flash") => (0.000075, 0.0003),
            _ => (0.00125, 0.005),
        };
        (input_tokens as f64 / 1_000.0) * in_per_1k
            + (output_tokens as f64 / 1_000.0) * out_per_1k
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_request(request: &ChatRequest) -> Result<(GeminiRequest, String), ClawzError> {
    let mut system_instruction: Option<GeminiContent> = None;
    let mut contents: Vec<GeminiContent> = Vec::new();

    // Handle system prompt
    if let Some(sys) = &request.system_prompt {
        system_instruction = Some(GeminiContent {
            role: "user".to_string(),
            parts: vec![GeminiPart::Text { text: sys.clone() }],
        });
    }

    for msg in &request.messages {
        let role = match msg.role {
            Role::System => {
                // Add to system instruction
                let text = match &msg.content {
                    MessageContent::Text(t) => t.clone(),
                    _ => serde_json::to_string(&msg.content)
                        .map_err(|e| ClawzError::Serialization(e.to_string()))?,
                };
                match system_instruction {
                    None => {
                        system_instruction = Some(GeminiContent {
                            role: "user".to_string(),
                            parts: vec![GeminiPart::Text { text }],
                        });
                    }
                    Some(ref mut si) => {
                        si.parts.push(GeminiPart::Text { text });
                    }
                }
                continue;
            }
            Role::User | Role::Tool => "user",
            Role::Assistant => "model",
        };

        let parts = convert_content_to_parts(&msg.content)?;
        contents.push(GeminiContent {
            role: role.to_string(),
            parts,
        });
    }

    let tools = build_tools(&request.tools);

    let body = GeminiRequest {
        contents,
        system_instruction,
        generation_config: Some(GeminiGenerationConfig {
            temperature: request.temperature,
            max_output_tokens: request.max_tokens,
            stop_sequences: request.stop.clone(),
        }),
        tools,
    };

    let model_path = request.model.clone();
    Ok((body, model_path))
}

fn convert_content_to_parts(content: &MessageContent) -> Result<Vec<GeminiPart>, ClawzError> {
    match content {
        MessageContent::Text(text) => Ok(vec![GeminiPart::Text { text: text.clone() }]),
        MessageContent::ToolCalls(calls) => Ok(calls
            .iter()
            .map(|tc| GeminiPart::FunctionCall {
                function_call: GeminiFunctionCall {
                    name: tc.name.clone(),
                    args: tc.arguments.clone(),
                },
            })
            .collect()),
        MessageContent::ToolResult(result) => Ok(vec![GeminiPart::FunctionResponse {
            function_response: GeminiFunctionResponse {
                name: "function".to_string(), // Gemini requires a name
                response: serde_json::json!({ "output": result.output }),
            },
        }]),
        MessageContent::Multimodal(parts) => {
            let gemini_parts: Vec<GeminiPart> = parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => GeminiPart::Text { text: text.clone() },
                    ContentPart::ImageBase64 { media_type, data } => {
                        GeminiPart::InlineData {
                            inline_data: GeminiInlineData {
                                mime_type: media_type.clone(),
                                data: data.clone(),
                            },
                        }
                    }
                    ContentPart::ImageUrl { url, .. } => {
                        // Gemini doesn't support raw URLs; treat as text annotation
                        GeminiPart::Text {
                            text: format!("[image: {}]", url),
                        }
                    }
                    ContentPart::AudioBase64 { media_type, data } => {
                        GeminiPart::InlineData {
                            inline_data: GeminiInlineData {
                                mime_type: media_type.clone(),
                                data: data.clone(),
                            },
                        }
                    }
                })
                .collect();
            Ok(gemini_parts)
        }
    }
}

fn build_tools(schemas: &[ToolSchema]) -> Vec<GeminiTool> {
    if schemas.is_empty() {
        return Vec::new();
    }
    vec![GeminiTool {
        function_declarations: schemas
            .iter()
            .map(|s| GeminiFunctionDeclaration {
                name: s.name.clone(),
                description: s.description.clone(),
                parameters: s.parameters.clone(),
            })
            .collect(),
    }]
}

fn parse_response(raw: GeminiResponse, model: &str) -> ChatResponse {
    let candidates = raw.candidates.unwrap_or_default();
    let choices: Vec<ChatChoice> = candidates
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let content = c
                .content
                .map(parse_gemini_content)
                .unwrap_or(MessageContent::Text(String::new()));

            let finish = c.finish_reason.as_deref().map(|s| match s {
                "STOP" | "FINISH_REASON_STOP" => "stop".to_string(),
                "MAX_TOKENS" => "length".to_string(),
                _ => s.to_lowercase(),
            });

            ChatChoice {
                index: i,
                message: Message {
                    id: Uuid::new_v4(),
                    role: Role::Assistant,
                    content,
                    created_at: Utc::now(),
                    name: None,
                },
                finish_reason: finish,
            }
        })
        .collect();

    let usage = raw.usage_metadata.map(|u| Usage {
        prompt_tokens: u.prompt_token_count.unwrap_or(0),
        completion_tokens: u.candidates_token_count.unwrap_or(0),
        total_tokens: u.total_token_count.unwrap_or(0),
    }).unwrap_or_default();

    ChatResponse {
        id: Uuid::new_v4().to_string(),
        model: raw.model_version.unwrap_or_else(|| model.to_string()),
        choices,
        usage,
        object: "chat.completion".to_string(),
        created: 0,
        created_at: Utc::now(),
    }
}

fn parse_gemini_content(gc: GeminiContent) -> MessageContent {
    // Check for function calls
    let function_calls: Vec<ToolCall> = gc
        .parts
        .iter()
        .filter_map(|p| {
            if let GeminiPart::FunctionCall { function_call } = p {
                Some(ToolCall {
                    id: Uuid::new_v4().to_string(),
                    name: function_call.name.clone(),
                    arguments: function_call.args.clone(),
                })
            } else {
                None
            }
        })
        .collect();

    if !function_calls.is_empty() {
        return MessageContent::ToolCalls(function_calls);
    }

    // Collect text parts
    let text = gc
        .parts
        .iter()
        .filter_map(|p| {
            if let GeminiPart::Text { text } = p {
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
    model: String,
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
                Ok(s) => s.to_string(),
                Err(e) => {
                    yield Err(ClawzError::Provider(format!("UTF-8: {e}")));
                    return;
                }
            };
            buf.push_str(&text);

            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim_end_matches('\r').to_string();
                buf = buf[nl + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                if let Some(data) = line.strip_prefix("data: ") {
                    match serde_json::from_str::<GeminiResponse>(data) {
                        Ok(partial) => {
                            let candidates = partial.candidates.unwrap_or_default();
                            for (i, candidate) in candidates.into_iter().enumerate() {
                                let delta_text = candidate.content.as_ref().and_then(|gc| {
                                    gc.parts.iter().find_map(|p| {
                                        if let GeminiPart::Text { text } = p {
                                            Some(text.clone())
                                        } else {
                                            None
                                        }
                                    })
                                });

                                let finish_reason = candidate.finish_reason.as_deref().map(|s| match s {
                                    "STOP" | "FINISH_REASON_STOP" => FinishReason::Stop,
                                    "MAX_TOKENS" => FinishReason::Length,
                                    _ => FinishReason::Stop,
                                });

                                let usage = if finish_reason.is_some() {
                                    partial.usage_metadata.as_ref().map(|u| Usage {
                                        prompt_tokens: u.prompt_token_count.unwrap_or(0),
                                        completion_tokens: u.candidates_token_count.unwrap_or(0),
                                        total_tokens: u.total_token_count.unwrap_or(0),
                                    })
                                } else {
                                    None
                                };

                                if delta_text.is_some() || finish_reason.is_some() {
                                    yield Ok(StreamChunk {
                                        id: Uuid::new_v4().to_string(),
                                        model: model.clone(),
                                        choice_index: i as u32,
                                        delta_text,
                                        delta_tool_call: None,
                                        finish_reason,
                                        usage,
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Gemini SSE parse error: {e} — {data}");
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
        let adapter = GeminiAdapter;
        assert_eq!(adapter.context_window("gemini-1.5-pro"), 1_000_000);
        assert_eq!(adapter.context_window("gemini-pro"), 32_768);
    }

    #[test]
    fn test_estimate_cost() {
        let adapter = GeminiAdapter;
        let cost = adapter.estimate_cost_usd("gemini-1.5-pro", 1_000, 1_000);
        assert!((cost - 0.00625).abs() < 0.0001);
    }

    #[test]
    fn test_build_request() {
        let request = ChatRequest {
            model: "gemini-1.5-flash".to_string(),
            messages: vec![Message::user("Hello")],
            ..Default::default()
        };
        let (body, model_path) = build_request(&request).unwrap();
        assert_eq!(model_path, "gemini-1.5-flash");
        assert_eq!(body.contents.len(), 1);
        assert_eq!(body.contents[0].role, "user");
    }

    #[test]
    fn test_parse_response_text() {
        let raw = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: "model".to_string(),
                    parts: vec![GeminiPart::Text {
                        text: "Hi there!".to_string(),
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(5),
                candidates_token_count: Some(3),
                total_token_count: Some(8),
            }),
            model_version: Some("gemini-1.5-flash".to_string()),
        };

        let response = parse_response(raw, "gemini-1.5-flash");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.first_text(), Some("Hi there!"));
        assert_eq!(response.usage.total_tokens, 8);
    }

    #[test]
    fn test_parse_response_function_call() {
        let raw = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: "model".to_string(),
                    parts: vec![GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            name: "get_weather".to_string(),
                            args: serde_json::json!({"location": "NYC"}),
                        },
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: None,
            model_version: None,
        };

        let response = parse_response(raw, "gemini-1.5-pro");
        let choice = &response.choices[0];
        assert!(matches!(&choice.message.content, MessageContent::ToolCalls(calls) if calls.len() == 1));
    }
}
