//! Provider configuration loaders.
//!
//! Supports both file-based (`provider.toml`) and environment-based
//! configuration. API keys may contain `${VAR_NAME}` placeholders that
//! are expanded at runtime by [`ProviderConfig::resolved_api_key()`].
//!
//! // Dependency: `super::router::ProviderRouterConfig` is the target type.

use std::path::Path;

use clawz_core::error::ClawzError;

use super::router::ProviderRouterConfig;

/// Load provider configuration from a TOML file.
///
/// Environment variable references in the format `${VAR_NAME}` within the
/// `api_key` fields are expanded at runtime by `ProviderConfig::resolved_api_key()`.
///
/// # Errors
///
/// Returns [`ClawzError::Config`] on missing file or parse failure.
pub async fn load_provider_config(
    path: impl AsRef<Path>,
) -> Result<ProviderRouterConfig, ClawzError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ClawzError::Config(format!("failed to read provider.toml: {e}")))?;

    toml::from_str(&content)
        .map_err(|e| ClawzError::Config(format!("failed to parse provider.toml: {e}")))
}

/// Persist a provider configuration to a TOML file.
///
/// # Errors
///
/// Returns [`ClawzError::Config`] on serialization failure or [`ClawzError::Io`]
/// on write failure.
pub async fn save_provider_config(
    config: &ProviderRouterConfig,
    path: impl AsRef<Path>,
) -> Result<(), ClawzError> {
    let content = toml::to_string_pretty(config)
        .map_err(|e| ClawzError::Config(format!("failed to serialize config: {e}")))?;

    tokio::fs::write(path, content)
        .await
        .map_err(|e| ClawzError::Io(std::io::Error::other(e.to_string())))
}

/// Build a `ProviderRouterConfig` from environment variables only.
///
/// Useful when no `provider.toml` is available (e.g. containerised deployments).
/// Falls back to sensible defaults for each known provider.
pub fn config_from_env() -> ProviderRouterConfig {
    use super::router::{AuthType, ProviderConfig};
    use std::collections::HashMap;

    let mut providers: HashMap<String, ProviderConfig> = HashMap::new();

    // OpenAI — the most common provider; requires OPENAI_API_KEY.
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            providers.insert(
                "openai".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec![
                        "gpt-4o".to_string(),
                        "gpt-4o-mini".to_string(),
                        "gpt-4-turbo".to_string(),
                        "gpt-4".to_string(),
                        "gpt-3.5-turbo".to_string(),
                        "o1".to_string(),
                        "o3".to_string(),
                    ],
                    auth_type: AuthType::Bearer,
                    ..Default::default()
                },
            );
        }
    }

    // Anthropic — requires ANTHROPIC_API_KEY.
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("ANTHROPIC_API_BASE")
                .unwrap_or_else(|_| "https://api.anthropic.com/v1".to_string());
            providers.insert(
                "anthropic".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec![
                        "claude-opus-4-5".to_string(),
                        "claude-sonnet-4-5".to_string(),
                        "claude-haiku-3".to_string(),
                        "claude-3-5-sonnet-20241022".to_string(),
                        "claude-3-5-haiku-20241022".to_string(),
                        "claude-3-opus-20240229".to_string(),
                    ],
                    auth_type: AuthType::ApiKey,
                    ..Default::default()
                },
            );
        }
    }

    // Google Gemini — requires GEMINI_API_KEY.
    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        if !key.is_empty() {
            providers.insert(
                "gemini".to_string(),
                ProviderConfig {
                    endpoint: "https://generativelanguage.googleapis.com/v1beta".to_string(),
                    api_key: key,
                    models: vec![
                        "gemini-1.5-pro".to_string(),
                        "gemini-1.5-flash".to_string(),
                        "gemini-2.0-flash".to_string(),
                    ],
                    ..Default::default()
                },
            );
        }
    }

    // DeepSeek — requires DEEPSEEK_API_KEY.
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("DEEPSEEK_API_BASE")
                .unwrap_or_else(|_| "https://api.deepseek.com/v1".to_string());
            providers.insert(
                "deepseek".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec![
                        "deepseek-chat".to_string(),
                        "deepseek-coder".to_string(),
                        "deepseek-reasoner".to_string(),
                    ],
                    ..Default::default()
                },
            );
        }
    }

    // Always add Ollama as a local option (no auth needed)
    let ollama_url =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
    providers.insert(
        "ollama".to_string(),
        ProviderConfig {
            endpoint: ollama_url,
            api_key: String::new(),
            models: vec![
                "llama3".to_string(),
                "llama3.1".to_string(),
                "mistral".to_string(),
                "codellama".to_string(),
            ],
            auth_type: AuthType::None,
            ..Default::default()
        },
    );

    // AWS Bedrock — uses AWS credential chain (env or profile).
    if std::env::var("AWS_ACCESS_KEY_ID").is_ok() || std::env::var("AWS_PROFILE").is_ok() {
        let mut extras = std::collections::HashMap::new();
        extras.insert(
            "region".to_string(),
            std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
        );
        providers.insert(
            "bedrock".to_string(),
            ProviderConfig {
                endpoint: "https://bedrock-runtime.amazonaws.com".to_string(),
                api_key: std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default(),
                models: vec![
                    "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
                    "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
                ],
                auth_type: AuthType::AwsSigV4,
                extras,
                ..Default::default()
            },
        );
    }

    // OpenRouter — OpenAI-compatible aggregator with 100+ models.
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("OPENROUTER_API_BASE")
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());
            providers.insert(
                "openrouter".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec![],
                    auth_type: AuthType::Bearer,
                    ..Default::default()
                },
            );
        }
    }

    // Groq — fast inference; OpenAI-compatible.
    if let Ok(key) = std::env::var("GROQ_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("GROQ_API_BASE")
                .unwrap_or_else(|_| "https://api.groq.com/openai/v1".to_string());
            providers.insert(
                "groq".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec![
                        "llama-3.3-70b-versatile".to_string(),
                        "mixtral-8x7b-32768".to_string(),
                    ],
                    auth_type: AuthType::Bearer,
                    ..Default::default()
                },
            );
        }
    }

    // Grok / xAI — OpenAI-compatible.
    if let Ok(key) = std::env::var("XAI_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("XAI_API_BASE")
                .unwrap_or_else(|_| "https://api.x.ai/v1".to_string());
            providers.insert(
                "xai".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec!["grok-2".to_string(), "grok-3".to_string()],
                    auth_type: AuthType::Bearer,
                    ..Default::default()
                },
            );
        }
    }

    // Together AI — OpenAI-compatible.
    if let Ok(key) = std::env::var("TOGETHER_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("TOGETHER_API_BASE")
                .unwrap_or_else(|_| "https://api.together.xyz/v1".to_string());
            providers.insert(
                "together".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec![],
                    auth_type: AuthType::Bearer,
                    ..Default::default()
                },
            );
        }
    }

    // Fireworks AI — OpenAI-compatible.
    if let Ok(key) = std::env::var("FIREWORKS_API_KEY") {
        if !key.is_empty() {
            let endpoint = std::env::var("FIREWORKS_API_BASE")
                .unwrap_or_else(|_| "https://api.fireworks.ai/inference/v1".to_string());
            providers.insert(
                "fireworks".to_string(),
                ProviderConfig {
                    endpoint,
                    api_key: key,
                    models: vec![],
                    auth_type: AuthType::Bearer,
                    ..Default::default()
                },
            );
        }
    }

    // Custom OpenAI-compatible provider — any endpoint.
    if let Ok(key) = std::env::var("CLAWZ_CUSTOM_LLM_KEY") {
        if !key.is_empty() {
            if let Ok(base) = std::env::var("CLAWZ_CUSTOM_LLM_BASE") {
                providers.insert(
                    "custom".to_string(),
                    ProviderConfig {
                        endpoint: base,
                        api_key: key,
                        models: vec![],
                        auth_type: AuthType::Bearer,
                        ..Default::default()
                    },
                );
            }
        }
    }

    ProviderRouterConfig {
        providers,
        ..Default::default()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::router::ProviderConfig;
    use super::*;
    use std::path::PathBuf;

    fn provider_toml_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("../../provider.toml");
        path
    }

    #[tokio::test]
    async fn test_load_provider_config() {
        let config = load_provider_config(provider_toml_path()).await.unwrap();
        assert!(config.providers.contains_key("openai"));
        assert!(config.providers.contains_key("anthropic"));
        assert!(config.providers.contains_key("gemini"));
    }

    #[tokio::test]
    async fn test_save_and_load_roundtrip() {
        let mut config = ProviderRouterConfig::default();
        config.providers.insert(
            "test".into(),
            ProviderConfig {
                endpoint: "https://test.example.com".into(),
                api_key: "test-key".into(),
                models: vec!["model-a".into()],
                ..Default::default()
            },
        );

        let temp_path = "/tmp/test_provider_clawz.toml";
        save_provider_config(&config, temp_path).await.unwrap();

        let loaded = load_provider_config(temp_path).await.unwrap();
        assert!(loaded.providers.contains_key("test"));
        let test = loaded.providers.get("test").unwrap();
        assert_eq!(test.endpoint, "https://test.example.com");
        assert_eq!(test.models, vec!["model-a"]);

        let _ = tokio::fs::remove_file(temp_path).await;
    }

    #[test]
    fn test_config_from_env_includes_ollama() {
        let config = config_from_env();
        assert!(config.providers.contains_key("ollama"));
    }

    #[tokio::test]
    async fn test_load_missing_file() {
        let result = load_provider_config("/nonexistent/path.toml").await;
        assert!(result.is_err());
    }
}
