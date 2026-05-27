//! DeepSeek adapter — uses the OpenAI-compatible API format.
//!
//! Base URL: https://api.deepseek.com/v1
//!
//! DeepSeek's API is fully OpenAI-compatible, so this adapter delegates
//! non-streaming and streaming calls to [`OpenAiAdapter`] after setting
//! the endpoint via configuration.
//!
//! // Dependency: `super::openai::OpenAiAdapter` for the actual HTTP work.

use std::pin::Pin;

use async_trait::async_trait;
use clawz_core::{error::ClawzError, types::message::*};
use futures_core::Stream;
use reqwest::Client;

use super::openai::OpenAiAdapter;
use super::{AdapterConfig, ProviderAdapter};

/// Adapter for the DeepSeek API.
pub struct DeepSeekAdapter;

#[async_trait]
impl ProviderAdapter for DeepSeekAdapter {
    fn provider_name(&self) -> &str {
        "deepseek"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec!["deepseek-chat", "deepseek-coder", "deepseek-reasoner"]
    }

    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        // DeepSeek is fully OpenAI-compatible — delegate to OpenAI adapter
        // but the endpoint and API key are from config
        OpenAiAdapter.chat(client, config, request).await
    }

    async fn chat_stream(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ClawzError>> + Send>>, ClawzError>
    {
        OpenAiAdapter.chat_stream(client, config, request).await
    }

    fn context_window(&self, model: &str) -> u32 {
        match model {
            "deepseek-chat" => 64_000,
            "deepseek-coder" => 16_000,
            "deepseek-reasoner" => 64_000,
            _ => 32_000,
        }
    }

    fn estimate_cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        // Pricing as of early 2025 (USD per 1K tokens)
        let (in_per_1k, out_per_1k) = match model {
            "deepseek-chat" => (0.00014, 0.00028),
            "deepseek-coder" => (0.00014, 0.00028),
            "deepseek-reasoner" => (0.00055, 0.00219),
            _ => (0.00014, 0.00028),
        };
        (input_tokens as f64 / 1_000.0) * in_per_1k + (output_tokens as f64 / 1_000.0) * out_per_1k
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_window() {
        let adapter = DeepSeekAdapter;
        assert_eq!(adapter.context_window("deepseek-chat"), 64_000);
        assert_eq!(adapter.context_window("deepseek-coder"), 16_000);
    }

    #[test]
    fn test_estimate_cost() {
        let adapter = DeepSeekAdapter;
        let cost = adapter.estimate_cost_usd("deepseek-chat", 1_000, 1_000);
        assert!(cost > 0.0 && cost < 0.001);
    }
}
