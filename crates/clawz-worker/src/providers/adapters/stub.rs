//! Local stub provider for smoke tests and offline development.
//!
//! Echoes the last user message without calling an external LLM API.

use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use clawz_core::{error::ClawzError, types::message::*};
use futures_core::Stream;
use futures_util::stream;
use reqwest::Client;
use uuid::Uuid;

use super::{AdapterConfig, ProviderAdapter};

pub struct StubAdapter;

impl StubAdapter {
    fn last_user_text(request: &ChatRequest) -> String {
        request
            .messages
            .iter()
            .rev()
            .find_map(|m| {
                if m.role == Role::User {
                    m.content.as_text().map(str::to_string)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "ready".to_string())
    }

    fn build_response(request: &ChatRequest) -> ChatResponse {
        let reply = format!("stub: {}", Self::last_user_text(request));
        let prompt_tokens = request
            .messages
            .iter()
            .filter_map(|m| m.content.as_text().map(str::len))
            .sum::<usize>()
            .max(1);
        let completion_tokens = reply.len().max(1);

        ChatResponse {
            id: format!("stub-{}", Uuid::new_v4()),
            model: request.model.clone(),
            choices: vec![ChatChoice {
                index: 0,
                message: Message::new(Role::Assistant, MessageContent::text(reply)),
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage::new(prompt_tokens, completion_tokens),
            object: "chat.completion".to_string(),
            created: Utc::now().timestamp() as u64,
            created_at: Utc::now(),
        }
    }
}

#[async_trait]
impl ProviderAdapter for StubAdapter {
    fn provider_name(&self) -> &str {
        "stub"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec!["*"]
    }

    async fn chat(
        &self,
        _client: &Client,
        _config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        Ok(Self::build_response(request))
    }

    async fn chat_stream(
        &self,
        _client: &Client,
        _config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ClawzError>> + Send>>, ClawzError>
    {
        let response = Self::build_response(request);
        let text = response.first_text().unwrap_or("").to_string();
        let chunk = StreamChunk {
            id: response.id.clone(),
            model: response.model.clone(),
            choice_index: 0,
            delta_text: Some(text),
            delta_tool_call: None,
            finish_reason: Some(FinishReason::Stop),
            usage: Some(response.usage.clone()),
        };
        Ok(Box::pin(stream::once(async move { Ok(chunk) })))
    }

    fn context_window(&self, _model: &str) -> u32 {
        128_000
    }

    fn estimate_cost_usd(&self, _model: &str, _input_tokens: u64, _output_tokens: u64) -> f64 {
        0.0
    }
}
