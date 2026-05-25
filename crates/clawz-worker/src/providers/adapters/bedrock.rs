//! AWS Bedrock adapter with SigV4 request signing.
//!
//! Bedrock Converse API: POST /model/{modelId}/converse
//! Authentication: AWS SigV4

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
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{http_error, AdapterConfig, ProviderAdapter};

type HmacSha256 = Hmac<Sha256>;

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct BedrockConverseRequest {
    messages: Vec<BedrockMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<BedrockSystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inference_config: Option<BedrockInferenceConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<BedrockToolConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BedrockMessage {
    role: String,
    content: Vec<BedrockContentBlock>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum BedrockContentBlock {
    Text(BedrockTextBlock),
    ToolUse(BedrockToolUseBlock),
    ToolResult(BedrockToolResultBlock),
}

#[derive(Debug, Serialize, Deserialize)]
struct BedrockTextBlock {
    text: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BedrockToolUseBlock {
    #[serde(rename = "toolUse")]
    tool_use: BedrockToolUse,
}

#[derive(Debug, Serialize, Deserialize)]
struct BedrockToolUse {
    #[serde(rename = "toolUseId")]
    tool_use_id: String,
    name: String,
    input: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct BedrockToolResultBlock {
    #[serde(rename = "toolResult")]
    tool_result: BedrockToolResult,
}

#[derive(Debug, Serialize, Deserialize)]
struct BedrockToolResult {
    #[serde(rename = "toolUseId")]
    tool_use_id: String,
    content: Vec<BedrockResultContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BedrockResultContent {
    text: String,
}

#[derive(Debug, Serialize)]
struct BedrockSystemBlock {
    text: String,
}

#[derive(Debug, Serialize)]
struct BedrockInferenceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct BedrockToolConfig {
    tools: Vec<BedrockTool>,
}

#[derive(Debug, Serialize)]
struct BedrockTool {
    #[serde(rename = "toolSpec")]
    tool_spec: BedrockToolSpec,
}

#[derive(Debug, Serialize)]
struct BedrockToolSpec {
    name: String,
    description: String,
    input_schema: BedrockInputSchema,
}

#[derive(Debug, Serialize)]
struct BedrockInputSchema {
    json: Value,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BedrockConverseResponse {
    output: BedrockOutput,
    usage: Option<BedrockUsage>,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BedrockOutput {
    message: BedrockMessage,
}

#[derive(Debug, Deserialize)]
struct BedrockUsage {
    input_tokens: usize,
    output_tokens: usize,
    total_tokens: usize,
}

// ── Stream response ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum BedrockStreamEvent {
    ContentBlockDelta {
        index: u32,
        delta: BedrockDelta,
    },
    MessageDelta {
        delta: BedrockMessageDelta,
        usage: Option<BedrockUsage>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum BedrockDelta {
    Text { text: String },
    ToolUse { input: String },
}

#[derive(Debug, Deserialize)]
struct BedrockMessageDelta {
    stop_reason: Option<String>,
}

// ── Adapter ───────────────────────────────────────────────────────────────────

pub struct BedrockAdapter;

#[async_trait]
impl ProviderAdapter for BedrockAdapter {
    fn provider_name(&self) -> &str {
        "bedrock"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec![
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-3-5-haiku-20241022-v1:0",
            "anthropic.claude-3-opus-20240229-v1:0",
            "anthropic.claude-3-sonnet-20240229-v1:0",
            "anthropic.claude-3-haiku-20240307-v1:0",
            "amazon.titan-text-lite-v1",
            "amazon.titan-text-express-v1",
            "meta.llama3-70b-instruct-v1:0",
            "meta.llama3-8b-instruct-v1:0",
            "mistral.mistral-large-2402-v1:0",
        ]
    }

    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        let region = config
            .extras
            .get("region")
            .cloned()
            .unwrap_or_else(|| std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string()));

        let aws_key = config
            .extras
            .get("access_key_id")
            .cloned()
            .unwrap_or_else(|| std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default());
        let aws_secret = config
            .extras
            .get("secret_access_key")
            .cloned()
            .unwrap_or_else(|| std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default());
        let aws_token = config
            .extras
            .get("session_token")
            .cloned()
            .or_else(|| std::env::var("AWS_SESSION_TOKEN").ok());

        let model_id = url_encode_model(&request.model);
        let path = format!("/model/{}/converse", model_id);
        let url = format!(
            "https://bedrock-runtime.{}.amazonaws.com{}",
            region, path
        );

        let body = build_request(request)?;
        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| ClawzError::Serialization(e.to_string()))?;

        let signed_req = sign_request(
            "POST",
            &url,
            &path,
            &body_bytes,
            &region,
            "bedrock",
            &aws_key,
            &aws_secret,
            aws_token.as_deref(),
        )?;

        let mut req_builder = client
            .post(&url)
            .header("content-type", "application/json")
            .body(body_bytes);

        for (k, v) in &signed_req.headers {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }

        let resp = req_builder.send().await.map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let raw: BedrockConverseResponse = resp
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
        let region = config
            .extras
            .get("region")
            .cloned()
            .unwrap_or_else(|| std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string()));

        let aws_key = config
            .extras
            .get("access_key_id")
            .cloned()
            .unwrap_or_else(|| std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default());
        let aws_secret = config
            .extras
            .get("secret_access_key")
            .cloned()
            .unwrap_or_else(|| std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default());
        let aws_token = config
            .extras
            .get("session_token")
            .cloned()
            .or_else(|| std::env::var("AWS_SESSION_TOKEN").ok());

        let model_id = url_encode_model(&request.model);
        let path = format!("/model/{}/converse-stream", model_id);
        let url = format!(
            "https://bedrock-runtime.{}.amazonaws.com{}",
            region, path
        );

        let body = build_request(request)?;
        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| ClawzError::Serialization(e.to_string()))?;

        let signed_req = sign_request(
            "POST",
            &url,
            &path,
            &body_bytes,
            &region,
            "bedrock",
            &aws_key,
            &aws_secret,
            aws_token.as_deref(),
        )?;

        let mut req_builder = client
            .post(&url)
            .header("content-type", "application/json")
            .body(body_bytes);

        for (k, v) in &signed_req.headers {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }

        let resp = req_builder.send().await.map_err(ClawzError::from)?;

        if !resp.status().is_success() {
            return Err(http_error(resp).await);
        }

        let model = request.model.clone();
        let byte_stream = resp.bytes_stream();
        let stream = parse_stream(byte_stream, model);
        Ok(Box::pin(stream))
    }

    fn context_window(&self, model: &str) -> u32 {
        if model.contains("claude") {
            200_000
        } else if model.contains("titan") {
            8_192
        } else if model.contains("llama3") {
            8_192
        } else {
            128_000
        }
    }

    fn estimate_cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        let (in_per_1k, out_per_1k) = if model.contains("claude-3-opus") {
            (0.015, 0.075)
        } else if model.contains("claude-3-5-sonnet") || model.contains("claude-3-sonnet") {
            (0.003, 0.015)
        } else if model.contains("claude-3-haiku") {
            (0.00025, 0.00125)
        } else {
            (0.005, 0.015)
        };
        (input_tokens as f64 / 1_000.0) * in_per_1k
            + (output_tokens as f64 / 1_000.0) * out_per_1k
    }
}

// ── Request builder ───────────────────────────────────────────────────────────

fn build_request(request: &ChatRequest) -> Result<BedrockConverseRequest, ClawzError> {
    let mut system_texts: Vec<String> = Vec::new();
    let mut messages: Vec<BedrockMessage> = Vec::new();

    if let Some(sys) = &request.system_prompt {
        system_texts.push(sys.clone());
    }

    for msg in &request.messages {
        if msg.role == Role::System {
            if let MessageContent::Text(t) = &msg.content {
                system_texts.push(t.clone());
            }
            continue;
        }

        let role = match msg.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => continue,
        }
        .to_string();

        let content = convert_content(&msg.content)?;
        messages.push(BedrockMessage { role, content });
    }

    let system = if system_texts.is_empty() {
        None
    } else {
        Some(
            system_texts
                .into_iter()
                .map(|t| BedrockSystemBlock { text: t })
                .collect(),
        )
    };

    let tool_config = if request.tools.is_empty() {
        None
    } else {
        Some(build_tools(&request.tools))
    };

    Ok(BedrockConverseRequest {
        messages,
        system,
        inference_config: Some(BedrockInferenceConfig {
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            stop_sequences: request.stop.clone(),
        }),
        tool_config,
    })
}

fn convert_content(content: &MessageContent) -> Result<Vec<BedrockContentBlock>, ClawzError> {
    match content {
        MessageContent::Text(text) => Ok(vec![BedrockContentBlock::Text(BedrockTextBlock {
            text: text.clone(),
        })]),
        MessageContent::ToolCalls(calls) => Ok(calls
            .iter()
            .map(|tc| {
                BedrockContentBlock::ToolUse(BedrockToolUseBlock {
                    tool_use: BedrockToolUse {
                        tool_use_id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.arguments.clone(),
                    },
                })
            })
            .collect()),
        MessageContent::ToolResult(result) => {
            Ok(vec![BedrockContentBlock::ToolResult(BedrockToolResultBlock {
                tool_result: BedrockToolResult {
                    tool_use_id: result.tool_call_id.clone(),
                    content: vec![BedrockResultContent {
                        text: result.output.clone(),
                    }],
                    status: if result.is_error {
                        Some("error".to_string())
                    } else {
                        None
                    },
                },
            })])
        }
        MessageContent::Multimodal(parts) => {
            let blocks: Vec<BedrockContentBlock> = parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(BedrockContentBlock::Text(BedrockTextBlock {
                        text: text.clone(),
                    })),
                    _ => None, // Binary data needs different handling
                })
                .collect();
            Ok(blocks)
        }
    }
}

fn build_tools(schemas: &[ToolSchema]) -> BedrockToolConfig {
    BedrockToolConfig {
        tools: schemas
            .iter()
            .map(|s| BedrockTool {
                tool_spec: BedrockToolSpec {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    input_schema: BedrockInputSchema {
                        json: s.parameters.clone(),
                    },
                },
            })
            .collect(),
    }
}

// ── Response parser ───────────────────────────────────────────────────────────

fn parse_response(raw: BedrockConverseResponse, model: &str) -> ChatResponse {
    let message = raw.output.message;
    let content = parse_message_content(message.content);

    let usage = raw.usage.map(|u| Usage {
        prompt_tokens: u.input_tokens,
        completion_tokens: u.output_tokens,
        total_tokens: u.total_tokens,
    }).unwrap_or_default();

    ChatResponse {
        id: Uuid::new_v4().to_string(),
        model: model.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: Message {
                id: Uuid::new_v4(),
                role: Role::Assistant,
                content,
                created_at: Utc::now(),
                name: None,
            },
            finish_reason: raw.stop_reason,
        }],
        usage,
        object: "chat.completion".to_string(),
        created: 0,
        created_at: Utc::now(),
    }
}

fn parse_message_content(blocks: Vec<BedrockContentBlock>) -> MessageContent {
    let tool_calls: Vec<ToolCall> = blocks
        .iter()
        .filter_map(|b| {
            if let BedrockContentBlock::ToolUse(tu) = b {
                Some(ToolCall {
                    id: tu.tool_use.tool_use_id.clone(),
                    name: tu.tool_use.name.clone(),
                    arguments: tu.tool_use.input.clone(),
                })
            } else {
                None
            }
        })
        .collect();

    if !tool_calls.is_empty() {
        return MessageContent::ToolCalls(tool_calls);
    }

    let text = blocks
        .iter()
        .filter_map(|b| {
            if let BedrockContentBlock::Text(t) = b {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");

    MessageContent::Text(text)
}

// ── Stream parser ─────────────────────────────────────────────────────────────

fn parse_stream(
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

                let data = if let Some(d) = line.strip_prefix("data: ") {
                    d.to_string()
                } else {
                    line.clone()
                };

                match serde_json::from_str::<BedrockStreamEvent>(&data) {
                    Ok(BedrockStreamEvent::ContentBlockDelta { index, delta }) => {
                        match delta {
                            BedrockDelta::Text { text } => {
                                yield Ok(StreamChunk {
                                    id: Uuid::new_v4().to_string(),
                                    model: model.clone(),
                                    choice_index: index,
                                    delta_text: Some(text),
                                    delta_tool_call: None,
                                    finish_reason: None,
                                    usage: None,
                                });
                            }
                            BedrockDelta::ToolUse { input } => {
                                yield Ok(StreamChunk {
                                    id: Uuid::new_v4().to_string(),
                                    model: model.clone(),
                                    choice_index: index,
                                    delta_text: None,
                                    delta_tool_call: Some(PartialToolCall {
                                        index,
                                        id: None,
                                        name: None,
                                        arguments_fragment: Some(input),
                                    }),
                                    finish_reason: None,
                                    usage: None,
                                });
                            }
                        }
                    }
                    Ok(BedrockStreamEvent::MessageDelta { delta, usage }) => {
                        let finish = delta.stop_reason.as_deref().map(|s| match s {
                            "end_turn" => FinishReason::Stop,
                            "max_tokens" => FinishReason::Length,
                            "tool_use" => FinishReason::ToolCalls,
                            _ => FinishReason::Stop,
                        });
                        let u = usage.map(|u| Usage {
                            prompt_tokens: u.input_tokens,
                            completion_tokens: u.output_tokens,
                            total_tokens: u.total_tokens,
                        });
                        yield Ok(StreamChunk {
                            id: Uuid::new_v4().to_string(),
                            model: model.clone(),
                            choice_index: 0,
                            delta_text: None,
                            delta_tool_call: None,
                            finish_reason: finish,
                            usage: u,
                        });
                    }
                    Ok(BedrockStreamEvent::Other) | Err(_) => {}
                }
            }
        }
    }
}

// ── AWS SigV4 Signing ─────────────────────────────────────────────────────────

struct SignedRequest {
    headers: Vec<(String, String)>,
}

fn sign_request(
    method: &str,
    url: &str,
    path: &str,
    body: &[u8],
    region: &str,
    service: &str,
    access_key: &str,
    secret_key: &str,
    session_token: Option<&str>,
) -> Result<SignedRequest, ClawzError> {
    let now = Utc::now();
    let date_time = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();

    let host = url
        .strip_prefix("https://")
        .and_then(|s| s.split('/').next())
        .unwrap_or("");

    // Hash the payload
    let body_hash = hex::encode(Sha256::digest(body));

    // Build canonical headers
    let mut canonical_headers = format!(
        "content-type:application/json\nhost:{}\nx-amz-date:{}\n",
        host, date_time
    );
    let mut signed_headers_list = "content-type;host;x-amz-date".to_string();

    if let Some(token) = session_token {
        canonical_headers.push_str(&format!("x-amz-security-token:{}\n", token));
        signed_headers_list.push_str(";x-amz-security-token");
    }

    let canonical_request = format!(
        "{}\n{}\n\n{}\n{}\n{}",
        method, path, canonical_headers, signed_headers_list, body_hash
    );

    let credential_scope = format!("{}/{}/{}/aws4_request", date, region, service);
    let canonical_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        date_time, credential_scope, canonical_hash
    );

    // Derive signing key
    let signing_key = derive_signing_key(secret_key, &date, region, service)?;
    let mut mac = HmacSha256::new_from_slice(&signing_key)
        .map_err(|e| ClawzError::Auth(format!("HMAC init error: {e}")))?;
    mac.update(string_to_sign.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());

    let auth_header = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        access_key, credential_scope, signed_headers_list, signature
    );

    let mut headers = vec![
        ("Authorization".to_string(), auth_header),
        ("x-amz-date".to_string(), date_time),
        ("x-amz-content-sha256".to_string(), body_hash),
    ];

    if let Some(token) = session_token {
        headers.push(("x-amz-security-token".to_string(), token.to_string()));
    }

    Ok(SignedRequest { headers })
}

fn derive_signing_key(
    secret_key: &str,
    date: &str,
    region: &str,
    service: &str,
) -> Result<Vec<u8>, ClawzError> {
    let date_key = hmac_sha256(format!("AWS4{}", secret_key).as_bytes(), date.as_bytes())?;
    let region_key = hmac_sha256(&date_key, region.as_bytes())?;
    let service_key = hmac_sha256(&region_key, service.as_bytes())?;
    hmac_sha256(&service_key, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, ClawzError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|e| ClawzError::Auth(format!("HMAC init error: {e}")))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn url_encode_model(model_id: &str) -> String {
    // Model IDs with colons and slashes must be percent-encoded
    model_id
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            c => c.to_string(),
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode_model() {
        assert_eq!(
            url_encode_model("anthropic.claude-3-sonnet-20240229-v1:0"),
            "anthropic.claude-3-sonnet-20240229-v1%3A0"
        );
    }

    #[test]
    fn test_context_window() {
        let adapter = BedrockAdapter;
        assert_eq!(adapter.context_window("anthropic.claude-3-opus-20240229-v1:0"), 200_000);
        assert_eq!(adapter.context_window("amazon.titan-text-lite-v1"), 8_192);
    }

    #[test]
    fn test_build_request() {
        let request = ChatRequest {
            model: "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
            messages: vec![
                Message::system("Be helpful."),
                Message::user("Hello"),
            ],
            ..Default::default()
        };
        let body = build_request(&request).unwrap();
        assert!(body.system.is_some());
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].role, "user");
    }

    #[test]
    fn test_derive_signing_key_does_not_panic() {
        let key = derive_signing_key("test-secret", "20240101", "us-east-1", "bedrock");
        assert!(key.is_ok());
        assert_eq!(key.unwrap().len(), 32);
    }

    #[test]
    fn test_parse_text_response() {
        let raw = BedrockConverseResponse {
            output: BedrockOutput {
                message: BedrockMessage {
                    role: "assistant".to_string(),
                    content: vec![BedrockContentBlock::Text(BedrockTextBlock {
                        text: "Hello!".to_string(),
                    })],
                },
            },
            usage: Some(BedrockUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            }),
            stop_reason: Some("end_turn".to_string()),
        };

        let response = parse_response(raw, "anthropic.claude-3-haiku");
        assert_eq!(response.usage.total_tokens, 15);
        assert_eq!(response.first_text(), Some("Hello!"));
    }
}
