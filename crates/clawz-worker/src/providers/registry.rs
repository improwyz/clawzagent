//! Provider registry: model-name resolution and fallback chains.
//!
//! [`ProviderRegistry`] maps exact model names to provider configurations
//! and constructs ordered fallback chains so the router can retry with
//! alternatives when a provider fails.
//!
//! // Dependency: `clawz_core::error::ClawzError` for unresolvable model names.

use std::collections::HashMap;
use std::sync::Arc;

use clawz_core::error::ClawzError;

use super::router::ProviderConfig;

// ── ProviderRegistry ──────────────────────────────────────────────────────────

/// Maps model names to provider names and holds provider configurations.
///
/// Resolution order (see [`resolve`]):
/// 1. Exact model match
/// 2. Provider-prefix match (e.g. `openai/gpt-4` → `openai`)
/// 3. Wildcard provider (empty model list)
pub struct ProviderRegistry {
    /// provider name -> config
    providers: HashMap<String, ProviderConfig>,
    /// exact model name -> provider name
    model_to_provider: HashMap<String, String>,
    /// provider name -> list of registered model names
    provider_models: HashMap<String, Vec<String>>,
}

impl ProviderRegistry {
    /// Build registry from a map of provider configs.
    pub fn new(configs: HashMap<String, ProviderConfig>) -> Self {
        let mut model_to_provider: HashMap<String, String> = HashMap::new();
        let mut provider_models: HashMap<String, Vec<String>> = HashMap::new();

        for (name, config) in &configs {
            let mut models_for_provider: Vec<String> = Vec::new();

            for model in &config.models {
                model_to_provider.insert(model.clone(), name.clone());
                models_for_provider.push(model.clone());
            }
            for fallback in &config.fallback_models {
                // fallback models also register under this provider, but don't
                // override an existing more-specific registration
                model_to_provider
                    .entry(fallback.clone())
                    .or_insert_with(|| name.clone());
                if !models_for_provider.contains(fallback) {
                    models_for_provider.push(fallback.clone());
                }
            }

            provider_models.insert(name.clone(), models_for_provider);
        }

        Self {
            providers: configs,
            model_to_provider,
            provider_models,
        }
    }

    // ── Resolution ────────────────────────────────────────────────────────────

    /// Resolve a model name to a provider name.
    ///
    /// Resolution order:
    /// 1. Exact model match
    /// 2. Prefix match against provider names (e.g. "openai/gpt-4" -> openai)
    /// 3. Wildcard provider (one with an empty model list)
    pub fn resolve(&self, model: &str) -> Result<String, ClawzError> {
        // 1. Exact model match
        if let Some(provider) = self.model_to_provider.get(model) {
            return Ok(provider.clone());
        }

        // 2. Provider-prefix: "openai/gpt-4" -> "openai"
        if let Some(slash_pos) = model.find('/') {
            let prefix = &model[..slash_pos];
            if self.providers.contains_key(prefix) {
                return Ok(prefix.to_string());
            }
        }

        // 3. Model name starts with provider name (e.g. "anthropic.claude-…" -> "bedrock"? no)
        //    Just match against explicit provider key prefix
        for name in self.providers.keys() {
            if model.starts_with(name.as_str()) {
                return Ok(name.clone());
            }
        }

        // 4. Wildcard provider (empty model list — accepts anything)
        for (name, config) in &self.providers {
            if config.models.is_empty() && config.fallback_models.is_empty() {
                return Ok(name.clone());
            }
        }

        Err(ClawzError::Provider(format!(
            "no provider found for model: {}",
            model
        )))
    }

    // ── Fallback chain ────────────────────────────────────────────────────────

    /// Return a list of (model, provider) fallback pairs for a given model.
    ///
    /// Used by the router to retry with alternatives when a provider fails.
    pub fn fallback_chain(&self, model: &str) -> Vec<(String, String)> {
        let mut chain: Vec<(String, String)> = Vec::new();

        // Find the primary provider
        let Ok(primary_provider) = self.resolve(model) else {
            return chain;
        };

        // Primary entry
        chain.push((model.to_string(), primary_provider.clone()));

        // Add fallback models from the same provider
        if let Some(config) = self.providers.get(&primary_provider) {
            for fallback in &config.fallback_models {
                if fallback != model {
                    chain.push((fallback.clone(), primary_provider.clone()));
                }
            }
        }

        chain
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    pub fn get_config(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }

    pub fn get_config_arc(&self, name: &str) -> Option<Arc<ProviderConfig>> {
        self.providers.get(name).cloned().map(Arc::new)
    }

    /// List all registered provider names.
    pub fn providers(&self) -> Vec<&String> {
        self.providers.keys().collect()
    }

    /// List all registered model names.
    pub fn all_models(&self) -> Vec<String> {
        let mut models: Vec<String> = self.model_to_provider.keys().cloned().collect();
        models.sort();
        models
    }

    /// List models for a specific provider.
    pub fn models_for_provider(&self, provider: &str) -> Vec<String> {
        self.provider_models
            .get(provider)
            .cloned()
            .unwrap_or_default()
    }

    /// Check if a model is registered.
    pub fn has_model(&self, model: &str) -> bool {
        self.model_to_provider.contains_key(model)
    }

    /// Check if a provider is registered.
    pub fn has_provider(&self, provider: &str) -> bool {
        self.providers.contains_key(provider)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::router::AuthType;

    fn make_config(models: Vec<&str>, fallbacks: Vec<&str>) -> ProviderConfig {
        ProviderConfig {
            endpoint: "https://example.com".to_string(),
            api_key: "key".to_string(),
            api_base: None,
            models: models.into_iter().map(String::from).collect(),
            auth_type: AuthType::Bearer,
            rate_limit_rpm: None,
            fallback_models: fallbacks.into_iter().map(String::from).collect(),
            extras: Default::default(),
        }
    }

    #[test]
    fn test_exact_model_resolution() {
        let mut configs = HashMap::new();
        configs.insert(
            "openai".into(),
            make_config(vec!["gpt-4", "gpt-3.5-turbo"], vec![]),
        );
        let registry = ProviderRegistry::new(configs);

        assert_eq!(registry.resolve("gpt-4").unwrap(), "openai");
        assert_eq!(registry.resolve("gpt-3.5-turbo").unwrap(), "openai");
    }

    #[test]
    fn test_fallback_model_resolution() {
        let mut configs = HashMap::new();
        configs.insert(
            "openai".into(),
            make_config(vec!["gpt-4"], vec!["gpt-3.5-turbo"]),
        );
        let registry = ProviderRegistry::new(configs);

        assert_eq!(registry.resolve("gpt-3.5-turbo").unwrap(), "openai");
    }

    #[test]
    fn test_prefix_resolution() {
        let mut configs = HashMap::new();
        configs.insert("openai".into(), make_config(vec![], vec![]));
        let registry = ProviderRegistry::new(configs);

        // "openai-gpt4" starts with "openai"
        let result = registry.resolve("openai-gpt4");
        assert!(result.is_ok());
    }

    #[test]
    fn test_slash_prefix_resolution() {
        let mut configs = HashMap::new();
        configs.insert("anthropic".into(), make_config(vec![], vec![]));
        let registry = ProviderRegistry::new(configs);

        let result = registry.resolve("anthropic/claude-3-opus");
        assert_eq!(result.unwrap(), "anthropic");
    }

    #[test]
    fn test_unknown_model_error() {
        let registry = ProviderRegistry::new(HashMap::new());
        let result = registry.resolve("unknown-model");
        assert!(result.is_err());
    }

    #[test]
    fn test_fallback_chain() {
        let mut configs = HashMap::new();
        configs.insert(
            "openai".into(),
            make_config(vec!["gpt-4"], vec!["gpt-3.5-turbo"]),
        );
        let registry = ProviderRegistry::new(configs);

        let chain = registry.fallback_chain("gpt-4");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].0, "gpt-4");
        assert_eq!(chain[1].0, "gpt-3.5-turbo");
    }

    #[test]
    fn test_all_models() {
        let mut configs = HashMap::new();
        configs.insert(
            "openai".into(),
            make_config(vec!["gpt-4", "gpt-4o"], vec![]),
        );
        configs.insert(
            "anthropic".into(),
            make_config(vec!["claude-3-opus"], vec![]),
        );
        let registry = ProviderRegistry::new(configs);

        let models = registry.all_models();
        assert_eq!(models.len(), 3);
        assert!(models.contains(&"gpt-4".to_string()));
        assert!(models.contains(&"claude-3-opus".to_string()));
    }

    #[test]
    fn test_has_model_and_provider() {
        let mut configs = HashMap::new();
        configs.insert("openai".into(), make_config(vec!["gpt-4"], vec![]));
        let registry = ProviderRegistry::new(configs);

        assert!(registry.has_model("gpt-4"));
        assert!(!registry.has_model("gpt-5"));
        assert!(registry.has_provider("openai"));
        assert!(!registry.has_provider("anthropic"));
    }
}
