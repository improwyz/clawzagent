//! Azure OpenAI adapter.
//!
//! URL pattern: `https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version={version}`
//! Auth: `api-key` header (not Bearer token).
//!
//! This adapter transforms the request into an OpenAI-compatible shape and
//! delegates to [`OpenAiAdapter`].
//!
//! // Dependency: `super::openai::OpenAiAdapter` for the actual HTTP work.

use std::pin::Pin;

use async_trait::async_trait;
use clawz_core::{error::ClawzError, types::message::*};
use futures_core::Stream;
use reqwest::Client;

use super::{AdapterConfig, ProviderAdapter};
use super::openai::OpenAiAdapter;

/// Adapter for Azure OpenAI Service.
pub struct AzureAdapter;

/// Default Azure API version used when none is specified in extras.
const DEFAULT_API_VERSION: &str = "2024-10-21";

#[async_trait]
impl ProviderAdapter for AzureAdapter {
    fn provider_name(&self) -> &str {
        "azure"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec![
            "gpt-4o",
            "gpt-4",
            "gpt-4-turbo",
            "gpt-35-turbo",
            "gpt-4o-mini",
        ]
    }

    async fn chat(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<ChatResponse, ClawzError> {
        let azure_config = build_azure_config(config, request);
        OpenAiAdapter.chat(client, &azure_config, request).await
    }

    async fn chat_stream(
        &self,
        client: &Client,
        config: &AdapterConfig,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ClawzError>> + Send>>, ClawzError>
    {
        let azure_config = build_azure_config(config, request);
        OpenAiAdapter.chat_stream(client, &azure_config, request).await
    }

    fn context_window(&self, model: &str) -> u32 {
        match model {
            m if m.starts_with("gpt-4o") => 128_000,
            m if m.starts_with("gpt-4-turbo") => 128_000,
            "gpt-4" => 8_192,
            m if m.starts_with("gpt-35-turbo") => 16_385,
            _ => 128_000,
        }
    }

    fn estimate_cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        // Azure pricing is similar to OpenAI with ~10% premium
        let base = super::openai::OpenAiAdapter.estimate_cost_usd(model, input_tokens, output_tokens);
        base * 1.1
    }
}

/// Build an [`AdapterConfig`] with the Azure-specific endpoint and auth headers.
fn build_azure_config(config: &AdapterConfig, request: &ChatRequest) -> AdapterConfig {
    // Azure endpoint pattern:
    // https://{resource}.openai.azure.com/openai/deployments/{deployment}
    // The deployment name is usually the model name in Azure
    let deployment = config
        .extras
        .get("deployment")
        .cloned()
        .unwrap_or_else(|| request.model.clone());

    let api_version = config
        .extras
        .get("api_version")
        .cloned()
        .unwrap_or_else(|| DEFAULT_API_VERSION.to_string());

    // Build the endpoint with deployment path and api-version query
    let base = config.endpoint.trim_end_matches('/');
    let endpoint = format!(
        "{}/openai/deployments/{}?api-version={}",
        base, deployment, api_version
    );

    // Azure uses "api-key" header instead of Bearer token
    let mut extra_headers = config.extra_headers.clone();
    extra_headers.push(("api-key".to_string(), config.api_key.clone()));

    AdapterConfig {
        endpoint,
        api_key: String::new(), // Azure uses api-key header, not Bearer
        extra_headers,
        extra_query: config.extra_query.clone(),
        extras: config.extras.clone(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_build_azure_config() {
        let mut extras = HashMap::new();
        extras.insert("deployment".to_string(), "my-gpt4".to_string());
        extras.insert("api_version".to_string(), "2024-02-15-preview".to_string());

        let config = AdapterConfig {
            endpoint: "https://myresource.openai.azure.com".to_string(),
            api_key: "my-azure-key".to_string(),
            extra_headers: Vec::new(),
            extra_query: Vec::new(),
            extras,
        };

        let request = ChatRequest {
            model: "gpt-4o".to_string(),
            ..Default::default()
        };

        let azure_config = build_azure_config(&config, &request);
        assert!(azure_config.endpoint.contains("my-gpt4"));
        assert!(azure_config.endpoint.contains("2024-02-15-preview"));
        // api-key header injected
        let has_api_key_header = azure_config
            .extra_headers
            .iter()
            .any(|(k, v)| k == "api-key" && v == "my-azure-key");
        assert!(has_api_key_header);
    }

    #[test]
    fn test_context_window() {
        let adapter = AzureAdapter;
        assert_eq!(adapter.context_window("gpt-4o"), 128_000);
        assert_eq!(adapter.context_window("gpt-4"), 8_192);
    }

    #[test]
    fn test_cost_premium_over_openai() {
        let adapter = AzureAdapter;
        let openai = super::super::openai::OpenAiAdapter;
        let azure_cost = adapter.estimate_cost_usd("gpt-4o", 1_000, 1_000);
        let openai_cost = openai.estimate_cost_usd("gpt-4o", 1_000, 1_000);
        assert!(azure_cost > openai_cost);
    }
}
