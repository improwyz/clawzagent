use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use clawz_core::{
    error::ClawzError,
    types::{message::*, tool::ToolCall},
};
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{AdapterConfig, ProviderAdapter, http_error};

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCall {
    function: OllamaFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaFunction {
    name: String,
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    model: String,
    message: Option<OllamaMessage>,
    done: bool,
    #[serde(default)]
    prompt_eval_count: usize,
    #[serde(default)]
    eval_count: usize,
}

// ── Adapter ───────────────────────────────────────────────────────────────────

pub struct OllamaAdapter;

#[async_trait]
impl ProviderAdapter for OllamaAdapter {
    fn provider_name(&self) -> &str {
        "ollama"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec![
            "llama3",
            "llama3:8b",
            "llama3:70b",
            "llama3.1",
            "llama3.1:8b",
            "llama3.1:70b",
            "mistral",
            "mistral:7b",
            "codellama",
            "codellama:7b",
            "codellama:34b",
            "phi3",
            "phi3:mini",
            "qwen2",
            "qwen2:7b",
            "deepseek-coder",
            "gemma2",
            "gemma2:9b",
            "gemma2:27b",
        ]
    }

    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        let body = build_request(request, false);
        let url = format!("{}/api/chat", config.endpoint.trim_end_matches('/'));

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let raw: OllamaResponse = resp
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
        let url = format!("{}/api/chat", config.endpoint.trim_end_matches('/'));

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let model = request.model.clone();
        let byte_stream = resp.bytes_stream();
        let stream = parse_ndjson_stream(byte_stream, model);
        Ok(Box::pin(stream))
    }

    fn context_window(&self, model: &str) -> u32 {
        match model {
            m if m.contains("llama3.1") => 128_000,
            m if m.contains("llama3") => 8_192,
            m if m.contains("mistral") => 32_768,
            m if m.contains("codellama") => 16_384,
            _ => 4_096,
        }
    }

    fn estimate_cost_usd(&self, _model: &str, _input_tokens: u64, _output_tokens: u64) -> f64 {
        // Ollama is local — no cost
        0.0
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_request(request: &ChatRequest, stream: bool) -> OllamaRequest {
    let mut messages: Vec<OllamaMessage> = Vec::new();

    // Add system prompt as system message
    if let Some(sys) = &request.system_prompt {
        messages.push(OllamaMessage {
            role: "system".to_string(),
            content: sys.clone(),
            tool_calls: None,
            images: None,
        });
    }

    for msg in &request.messages {
        let role = msg.role.as_str().to_string();
        let (content, tool_calls, images) = extract_ollama_content(&msg.content);
        messages.push(OllamaMessage {
            role,
            content,
            tool_calls,
            images,
        });
    }

    // Build tools in OpenAI format (Ollama supports this)
    let tools = request
        .tools
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
        .collect();

    OllamaRequest {
        model: request.model.clone(),
        messages,
        stream,
        options: Some(OllamaOptions {
            temperature: request.temperature,
            num_predict: request.max_tokens,
            stop: request.stop.clone(),
        }),
        tools,
    }
}

fn extract_ollama_content(
    content: &MessageContent,
) -> (String, Option<Vec<OllamaToolCall>>, Option<Vec<String>>) {
    match content {
        MessageContent::Text(t) => (t.clone(), None, None),
        MessageContent::ToolCalls(calls) => {
            let tool_calls: Vec<OllamaToolCall> = calls
                .iter()
                .map(|tc| OllamaToolCall {
                    function: OllamaFunction {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                })
                .collect();
            ("".to_string(), Some(tool_calls), None)
        }
        MessageContent::ToolResult(r) => (r.output.clone(), None, None),
        MessageContent::Multimodal(parts) => {
            let mut text_parts: Vec<String> = Vec::new();
            let mut images: Vec<String> = Vec::new();
            for p in parts {
                match p {
                    ContentPart::Text { text } => text_parts.push(text.clone()),
                    ContentPart::ImageBase64 { data, .. } => images.push(data.clone()),
                    ContentPart::ImageUrl { url, .. } => text_parts.push(format!("[image: {url}]")),
                    ContentPart::AudioBase64 { .. } => {}
                }
            }
            let imgs = if images.is_empty() {
                None
            } else {
                Some(images)
            };
            (text_parts.join(""), None, imgs)
        }
    }
}

fn parse_response(raw: OllamaResponse) -> ChatResponse {
    let content = raw
        .message
        .as_ref()
        .map(|m| {
            if let Some(tool_calls) = &m.tool_calls {
                if !tool_calls.is_empty() {
                    let calls: Vec<ToolCall> = tool_calls
                        .iter()
                        .map(|tc| ToolCall {
                            id: Uuid::new_v4().to_string(),
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        })
                        .collect();
                    return MessageContent::ToolCalls(calls);
                }
            }
            MessageContent::Text(m.content.clone())
        })
        .unwrap_or(MessageContent::Text(String::new()));

    let finish_reason = if raw.done {
        Some("stop".to_string())
    } else {
        None
    };

    ChatResponse {
        id: Uuid::new_v4().to_string(),
        model: raw.model.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: Message {
                id: Uuid::new_v4(),
                role: Role::Assistant,
                content,
                created_at: Utc::now(),
                name: None,
            },
            finish_reason,
        }],
        usage: Usage {
            prompt_tokens: raw.prompt_eval_count,
            completion_tokens: raw.eval_count,
            total_tokens: raw.prompt_eval_count + raw.eval_count,
        },
        object: "chat.completion".to_string(),
        created: 0,
        created_at: Utc::now(),
    }
}

/// Ollama streams NDJSON (one JSON object per line).
fn parse_ndjson_stream(
    byte_stream: impl futures_core::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    model: String,
) -> impl Stream<Item = Result<StreamChunk, ClawzError>> + Send {
    let mut buf = String::new();
    let stream_id = Uuid::new_v4().to_string();

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
                let line = buf[..nl].trim().to_string();
                buf = buf[nl + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<OllamaResponse>(&line) {
                    Ok(chunk_resp) => {
                        let delta_text = chunk_resp.message.as_ref().map(|m| m.content.clone());

                        let finish_reason = if chunk_resp.done {
                            Some(FinishReason::Stop)
                        } else {
                            None
                        };

                        let usage = if chunk_resp.done {
                            Some(Usage {
                                prompt_tokens: chunk_resp.prompt_eval_count,
                                completion_tokens: chunk_resp.eval_count,
                                total_tokens: chunk_resp.prompt_eval_count + chunk_resp.eval_count,
                            })
                        } else {
                            None
                        };

                        if delta_text.is_some() || finish_reason.is_some() {
                            yield Ok(StreamChunk {
                                id: stream_id.clone(),
                                model: model.clone(),
                                choice_index: 0,
                                delta_text,
                                delta_tool_call: None,
                                finish_reason,
                                usage,
                            });
                        }

                        if chunk_resp.done {
                            return;
                        }
                    }
                    Err(e) => {
                        log::warn!("Ollama NDJSON parse error: {e} — {line}");
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
    fn test_zero_cost() {
        let adapter = OllamaAdapter;
        assert_eq!(
            adapter.estimate_cost_usd("llama3", 1_000_000, 1_000_000),
            0.0
        );
    }

    #[test]
    fn test_build_request() {
        let request = ChatRequest {
            model: "llama3".to_string(),
            messages: vec![Message::user("Hi")],
            temperature: Some(0.5),
            ..Default::default()
        };
        let body = build_request(&request, false);
        assert_eq!(body.model, "llama3");
        assert!(!body.stream);
        assert_eq!(body.messages[0].role, "user");
    }

    #[test]
    fn test_parse_response() {
        let raw = OllamaResponse {
            model: "llama3".to_string(),
            message: Some(OllamaMessage {
                role: "assistant".to_string(),
                content: "Hello!".to_string(),
                tool_calls: None,
                images: None,
            }),
            done: true,
            prompt_eval_count: 5,
            eval_count: 3,
        };

        let response = parse_response(raw);
        assert_eq!(response.model, "llama3");
        assert_eq!(response.first_text(), Some("Hello!"));
        assert_eq!(response.usage.total_tokens, 8);
    }
}
