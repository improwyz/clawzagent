//! Intelligent LLM request router with circuit breaker, retries, and cost tracking.
//!
//! [`ProviderRouter`] is the main entry point for dispatching chat requests.
//! It coordinates:
//!
//! - **Registry** — model → provider resolution
//! - **Cost tracker** — per-call cost recording and budget enforcement
//! - **Circuit breaker** — per-provider failure isolation
//! - **Retry / backoff** — exponential backoff with jitter for transient errors
//!
//! // Dependency: `clawz_core::traits::Provider` is the ultimate contract that
//! // adapters satisfy for the worker.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clawz_core::{
    error::ClawzError,
    types::{
        cost::BudgetConfig,
        message::{ChatRequest, ChatResponse, StreamChunk},
    },
};
use futures_core::Stream;
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{
    adapters::{
        anthropic::AnthropicAdapter, azure::AzureAdapter, bedrock::BedrockAdapter,
        deepseek::DeepSeekAdapter, gemini::GeminiAdapter, ollama::OllamaAdapter,
        openai::OpenAiAdapter, AdapterConfig, ProviderAdapter,
    },
    cost::CostTracker,
    registry::ProviderRegistry,
};

// ── Config structs ────────────────────────────────────────────────────────────

/// Top-level router configuration loaded from TOML or environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRouterConfig {
    /// Map of provider name → provider-specific configuration.
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    /// Retry / circuit-breaker settings.
    #[serde(default = "default_reliability")]
    pub reliability: ReliabilityConfig,
    /// Optional spending caps.
    #[serde(default)]
    pub budget: Option<BudgetConfig>,
}

fn default_reliability() -> ReliabilityConfig {
    ReliabilityConfig {
        max_retries: 3,
        base_delay_ms: 500,
        max_delay_ms: 30_000,
        exponential_base: 2.0,
        jitter: 0.1,
        circuit_breaker_threshold: 5,
        circuit_breaker_timeout_secs: 30,
    }
}

impl Default for ProviderRouterConfig {
    fn default() -> Self {
        Self {
            providers: HashMap::new(),
            reliability: default_reliability(),
            budget: None,
        }
    }
}

/// Per-provider HTTP configuration.
///
/// The `api_key` field may contain a literal key or a `${VAR_NAME}`
/// placeholder expanded at runtime by [`resolved_api_key`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Base endpoint URL (e.g. `https://api.openai.com/v1`).
    pub endpoint: String,
    /// API key value or env-var reference like `${OPENAI_API_KEY}`.
    pub api_key: String,
    /// Optional override for the API base path (used by Azure).
    #[serde(default)]
    pub api_base: Option<String>,
    /// List of primary models offered by this provider.
    #[serde(default)]
    pub models: Vec<String>,
    /// Authentication scheme for HTTP requests.
    #[serde(default)]
    pub auth_type: AuthType,
    /// Optional rate-limit in requests per minute.
    #[serde(default)]
    pub rate_limit_rpm: Option<u32>,
    /// Models to try when the primary model fails.
    #[serde(default)]
    pub fallback_models: Vec<String>,
    /// Provider-specific extras (region, deployment, api_version …)
    #[serde(default)]
    pub extras: HashMap<String, String>,
}

impl ProviderConfig {
    /// Resolve the API key, expanding environment-variable references.
    pub fn resolved_api_key(&self) -> String {
        if let Some(var_name) = self.api_key.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            std::env::var(var_name).unwrap_or_default()
        } else {
            self.api_key.clone()
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            api_key: String::new(),
            api_base: None,
            models: Vec::new(),
            auth_type: AuthType::Bearer,
            rate_limit_rpm: None,
            fallback_models: Vec::new(),
            extras: HashMap::new(),
        }
    }
}

/// Authentication schemes supported by the router.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthType {
    /// HTTP `Authorization: Bearer <token>` header.
    #[default]
    Bearer,
    /// HTTP `x-api-key` header.
    ApiKey,
    /// AWS Signature Version 4 (used by Bedrock).
    AwsSigV4,
    /// No authentication (e.g. local Ollama).
    None,
}

/// Retry and circuit-breaker tuning parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReliabilityConfig {
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_base_delay")]
    pub base_delay_ms: u64,
    #[serde(default = "default_max_delay")]
    pub max_delay_ms: u64,
    #[serde(default = "default_exp_base")]
    pub exponential_base: f64,
    #[serde(default = "default_jitter")]
    pub jitter: f64,
    #[serde(default = "default_cb_threshold")]
    pub circuit_breaker_threshold: u32,
    #[serde(default = "default_cb_timeout")]
    pub circuit_breaker_timeout_secs: u64,
}

fn default_max_retries() -> u32 { 3 }
fn default_base_delay() -> u64 { 500 }
fn default_max_delay() -> u64 { 30_000 }
fn default_exp_base() -> f64 { 2.0 }
fn default_jitter() -> f64 { 0.1 }
fn default_cb_threshold() -> u32 { 5 }
fn default_cb_timeout() -> u64 { 30 }

// ── Circuit breaker ───────────────────────────────────────────────────────────

/// Finite-state machine for per-provider circuit breaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests allowed.
    Closed,
    /// Failure threshold exceeded — requests blocked until timeout elapses.
    Open { opened_at: Instant },
    /// Probing after timeout — one successful request will close the circuit.
    HalfOpen,
}

/// Per-provider failure-tracking circuit breaker.
#[derive(Debug)]
struct CircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    threshold: u32,
    timeout: Duration,
}

impl CircuitBreaker {
    fn new(threshold: u32, timeout_secs: u64) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            threshold,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Returns `true` if the circuit is open (requests should NOT proceed).
    fn is_open(&mut self) -> bool {
        match &self.state {
            CircuitState::Closed | CircuitState::HalfOpen => false,
            CircuitState::Open { opened_at } => {
                if opened_at.elapsed() >= self.timeout {
                    log::info!("Circuit breaker half-opening");
                    self.state = CircuitState::HalfOpen;
                    self.failure_count = 0;
                    false
                } else {
                    true
                }
            }
        }
    }

    fn record_success(&mut self) {
        self.failure_count = 0;
        self.success_count += 1;
        if self.state == CircuitState::HalfOpen {
            log::info!("Circuit breaker closing after successful probe");
            self.state = CircuitState::Closed;
        }
    }

    fn record_failure(&mut self) {
        self.failure_count += 1;
        self.success_count = 0;
        if self.failure_count >= self.threshold {
            log::warn!(
                "Circuit breaker opening after {} failures",
                self.failure_count
            );
            self.state = CircuitState::Open {
                opened_at: Instant::now(),
            };
        }
    }
}

// ── ProviderRouter ────────────────────────────────────────────────────────────

/// Central router that dispatches chat requests to the correct provider adapter.
///
/// Holds a [`ProviderRegistry`], a [`CostTracker`], and per-provider circuit
/// breakers. Clone the `Arc<ProviderRouter>` to share across tasks.
pub struct ProviderRouter {
    /// Shared HTTP client for all adapter calls.
    client: Client,
    /// Model → provider registry with fallback chains.
    pub registry: ProviderRegistry,
    /// Retry / backoff parameters.
    reliability: ReliabilityConfig,
    /// Tracks spend and enforces budgets.
    pub cost_tracker: CostTracker,
    /// Per-provider circuit breakers keyed by provider name.
    circuit_breakers: Arc<RwLock<HashMap<String, CircuitBreaker>>>,
}

impl ProviderRouter {
    /// Initialise the router from a configuration.
    ///
    /// Creates the HTTP client, builds the registry, initialises the cost
    /// tracker, and pre-populates circuit breakers for every configured provider.
    pub async fn new(config: ProviderRouterConfig) -> Result<Self, ClawzError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| ClawzError::Internal(e.to_string()))?;

        let registry = ProviderRegistry::new(config.providers.clone());
        let cost_tracker = CostTracker::with_budget(config.budget);

        // Pre-populate circuit breakers for known providers
        let mut breakers: HashMap<String, CircuitBreaker> = HashMap::new();
        for name in config.providers.keys() {
            breakers.insert(
                name.clone(),
                CircuitBreaker::new(
                    config.reliability.circuit_breaker_threshold,
                    config.reliability.circuit_breaker_timeout_secs,
                ),
            );
        }

        Ok(Self {
            client,
            registry,
            reliability: config.reliability,
            cost_tracker,
            circuit_breakers: Arc::new(RwLock::new(breakers)),
        })
    }

    // ── Public routing API ────────────────────────────────────────────────────

    /// Route a chat request to the appropriate provider.
    ///
    /// Steps:
    /// 1. Check budget.
    /// 2. Resolve fallback chain for the requested model.
    /// 3. Iterate the chain, skipping providers with an open circuit breaker.
    /// 4. Execute with retry; on success record cost and return.
    pub async fn route(&self, request: ChatRequest) -> Result<ChatResponse, ClawzError> {
        self.cost_tracker.check_budget().await?;

        let chain = self.registry.fallback_chain(&request.model);
        if chain.is_empty() {
            return Err(ClawzError::Provider(format!(
                "no provider for model: {}",
                request.model
            )));
        }

        let mut last_err: Option<ClawzError> = None;

        for (model, provider_name) in &chain {
            // Check circuit breaker
            {
                let mut breakers = self.circuit_breakers.write().await;
                let breaker = breakers.entry(provider_name.clone()).or_insert_with(|| {
                    CircuitBreaker::new(
                        self.reliability.circuit_breaker_threshold,
                        self.reliability.circuit_breaker_timeout_secs,
                    )
                });
                if breaker.is_open() {
                    log::warn!("Circuit open for provider {provider_name}, skipping");
                    last_err = Some(ClawzError::Provider(format!(
                        "circuit open for {provider_name}"
                    )));
                    continue;
                }
            }

            let mut req = request.clone();
            req.model = model.clone();

            match self.execute_with_retry(&req, provider_name).await {
                Ok(response) => {
                    let mut breakers = self.circuit_breakers.write().await;
                    if let Some(b) = breakers.get_mut(provider_name) {
                        b.record_success();
                    }
                    self.cost_tracker.record(provider_name, None, &response).await;
                    return Ok(response);
                }
                Err(e) => {
                    log::warn!("Provider {provider_name} failed: {e}");
                    {
                        let mut breakers = self.circuit_breakers.write().await;
                        if let Some(b) = breakers.get_mut(provider_name) {
                            b.record_failure();
                        }
                    }
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| ClawzError::Provider("all providers failed".into())))
    }

    /// Route a streaming chat request.
    ///
    /// Unlike [`route`], streaming does not currently retry across the fallback
    /// chain; it resolves a single provider and returns the stream.
    pub async fn route_stream(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ClawzError>> + Send>>, ClawzError>
    {
        self.cost_tracker.check_budget().await?;

        let provider_name = self.registry.resolve(&request.model)?;

        // Check circuit breaker
        {
            let mut breakers = self.circuit_breakers.write().await;
            let breaker = breakers.entry(provider_name.clone()).or_insert_with(|| {
                CircuitBreaker::new(
                    self.reliability.circuit_breaker_threshold,
                    self.reliability.circuit_breaker_timeout_secs,
                )
            });
            if breaker.is_open() {
                return Err(ClawzError::Provider(format!(
                    "circuit open for {provider_name}"
                )));
            }
        }

        let adapter = self.get_adapter(&provider_name)?;
        let config = self.build_adapter_config(&provider_name)?;

        let stream = adapter
            .chat_stream(&self.client, &config, &request)
            .await
            .map_err(|e| {
                log::warn!("Stream failed for {provider_name}: {e}");
                e
            })?;

        Ok(stream)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Execute a chat request with exponential-backoff retries.
    async fn execute_with_retry(
        &self,
        request: &ChatRequest,
        provider_name: &str,
    ) -> Result<ChatResponse, ClawzError> {
        let adapter = self.get_adapter(provider_name)?;
        let config = self.build_adapter_config(provider_name)?;

        let mut last_err: Option<ClawzError> = None;

        for attempt in 0..=self.reliability.max_retries {
            if attempt > 0 {
                let delay = self.backoff_delay(attempt - 1);
                log::debug!(
                    "Retry {}/{} for {provider_name} after {:?}",
                    attempt,
                    self.reliability.max_retries,
                    delay
                );
                tokio::time::sleep(delay).await;
            }

            match adapter.chat(&self.client, &config, request).await {
                Ok(response) => return Ok(response),
                Err(ClawzError::RateLimited { retry_after_secs }) => {
                    log::warn!("{provider_name} rate-limited, retry after {retry_after_secs}s");
                    if attempt < self.reliability.max_retries {
                        tokio::time::sleep(Duration::from_secs(retry_after_secs)).await;
                    }
                    last_err = Some(ClawzError::RateLimited { retry_after_secs });
                }
                Err(ClawzError::Auth(_)) => {
                    // Auth errors should NOT be retried
                    return Err(ClawzError::Auth(format!(
                        "auth error for {provider_name}"
                    )));
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ClawzError::Provider(format!("all retries exhausted for {provider_name}"))
        }))
    }

    /// Compute the backoff delay for a given retry attempt.
    ///
    /// Uses exponential backoff with random jitter to avoid thundering herds.
    fn backoff_delay(&self, attempt: u32) -> Duration {
        let base = self.reliability.base_delay_ms as f64;
        let delay = base * self.reliability.exponential_base.powi(attempt as i32);
        let jitter_amount = delay * self.reliability.jitter * rand::thread_rng().r#gen::<f64>();
        let total = (delay + jitter_amount).min(self.reliability.max_delay_ms as f64);
        Duration::from_millis(total as u64)
    }

    /// Select the correct [`ProviderAdapter`] for a provider name.
    ///
    /// Falls back to endpoint-pattern matching for unknown provider names so
    /// that custom OpenAI-compatible endpoints work out of the box.
    fn get_adapter(&self, provider_name: &str) -> Result<Box<dyn ProviderAdapter>, ClawzError> {
        let adapter: Box<dyn ProviderAdapter> = match provider_name {
            "openai" => Box::new(OpenAiAdapter),
            "anthropic" => Box::new(AnthropicAdapter),
            "gemini" => Box::new(GeminiAdapter),
            "bedrock" => Box::new(BedrockAdapter),
            "ollama" => Box::new(OllamaAdapter),
            "deepseek" => Box::new(DeepSeekAdapter),
            "azure" => Box::new(AzureAdapter),
            other => {
                // Try to infer adapter from endpoint pattern in config
                if let Some(cfg) = self.registry.get_config(other) {
                    if cfg.endpoint.contains("anthropic.com") {
                        Box::new(AnthropicAdapter)
                    } else if cfg.endpoint.contains("generativelanguage.googleapis.com") {
                        Box::new(GeminiAdapter)
                    } else if cfg.endpoint.contains("amazonaws.com") {
                        Box::new(BedrockAdapter)
                    } else if cfg.endpoint.contains("localhost:11434") {
                        Box::new(OllamaAdapter)
                    } else if cfg.endpoint.contains("deepseek.com") {
                        Box::new(DeepSeekAdapter)
                    } else if cfg.endpoint.contains("openai.azure.com") {
                        Box::new(AzureAdapter)
                    } else {
                        // Default: treat as OpenAI-compatible
                        Box::new(OpenAiAdapter)
                    }
                } else {
                    return Err(ClawzError::Provider(format!(
                        "unknown provider: {other}"
                    )));
                }
            }
        };
        Ok(adapter)
    }

    /// Build an [`AdapterConfig`] from the stored [`ProviderConfig`].
    fn build_adapter_config(&self, provider_name: &str) -> Result<AdapterConfig, ClawzError> {
        let config = self.registry.get_config(provider_name).ok_or_else(|| {
            ClawzError::Provider(format!("config not found for provider: {provider_name}"))
        })?;

        Ok(AdapterConfig {
            endpoint: config
                .api_base
                .clone()
                .unwrap_or_else(|| config.endpoint.clone()),
            api_key: config.resolved_api_key(),
            extra_headers: Vec::new(),
            extra_query: Vec::new(),
            extras: config.extras.clone(),
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_router_initialization() {
        let config = ProviderRouterConfig::default();
        let router = ProviderRouter::new(config).await.unwrap();
        assert!(router.registry.providers().is_empty());
    }

    #[tokio::test]
    async fn test_router_with_providers() {
        let mut config = ProviderRouterConfig::default();
        config.providers.insert(
            "openai".into(),
            ProviderConfig {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: "test-key".into(),
                models: vec!["gpt-4o".into()],
                ..Default::default()
            },
        );
        let router = ProviderRouter::new(config).await.unwrap();

        assert_eq!(router.registry.providers().len(), 1);
        let resolved = router.registry.resolve("gpt-4o");
        assert!(resolved.is_ok());
        assert_eq!(resolved.unwrap(), "openai");
    }

    #[tokio::test]
    async fn test_backoff_delay_increases() {
        let config = ProviderRouterConfig::default();
        let router = ProviderRouter::new(config).await.unwrap();

        let d0 = router.backoff_delay(0);
        let d1 = router.backoff_delay(1);
        let d2 = router.backoff_delay(2);

        // Due to jitter these could overlap, but trend should be increasing
        assert!(d0.as_millis() >= 400); // ~500ms base
        assert!(d1.as_millis() > d0.as_millis() / 2); // exponential growth
        assert!(d2.as_millis() >= d0.as_millis()); // never decreases much
    }

    #[tokio::test]
    async fn test_backoff_capped_at_max() {
        let config = ProviderRouterConfig::default();
        let router = ProviderRouter::new(config).await.unwrap();

        let delay = router.backoff_delay(20); // very large attempt
        assert!(delay.as_millis() <= 30_000 + 5_000); // allow small jitter overshoot
    }

    #[test]
    fn test_resolved_api_key_env_var() {
        let config = ProviderConfig {
            api_key: "${TEST_CLAWZ_KEY_12345}".to_string(),
            ..Default::default()
        };
        // Without the env var set it should return empty string
        let key = config.resolved_api_key();
        assert_eq!(key, "");

        // With it set
        // Safety: test-only, single-threaded context
        unsafe {
            std::env::set_var("TEST_CLAWZ_KEY_12345", "secret");
        }
        let key = config.resolved_api_key();
        assert_eq!(key, "secret");
        unsafe {
            std::env::remove_var("TEST_CLAWZ_KEY_12345");
        }
    }

    #[test]
    fn test_resolved_api_key_literal() {
        let config = ProviderConfig {
            api_key: "sk-literal-key".to_string(),
            ..Default::default()
        };
        assert_eq!(config.resolved_api_key(), "sk-literal-key");
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let mut cb = CircuitBreaker::new(3, 30);
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open()); // not yet
        cb.record_failure(); // 3rd failure
        assert!(cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_success_resets() {
        let mut cb = CircuitBreaker::new(3, 30);
        cb.record_failure();
        cb.record_failure();
        cb.record_success(); // resets failure count
        cb.record_failure();
        assert!(!cb.is_open()); // still only 1 failure
    }

    #[test]
    fn test_get_adapter_known_providers() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let router = rt.block_on(async {
            ProviderRouter::new(ProviderRouterConfig::default()).await.unwrap()
        });

        assert!(router.get_adapter("openai").is_ok());
        assert!(router.get_adapter("anthropic").is_ok());
        assert!(router.get_adapter("gemini").is_ok());
        assert!(router.get_adapter("bedrock").is_ok());
        assert!(router.get_adapter("ollama").is_ok());
        assert!(router.get_adapter("deepseek").is_ok());
        assert!(router.get_adapter("azure").is_ok());
    }
}
