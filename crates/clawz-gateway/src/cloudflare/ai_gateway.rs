//! Cloudflare AI Gateway — LLM request proxying with observability.
//!
//! The AI Gateway sits between the ClawZ gateway and upstream LLM providers
//! (OpenAI, Anthropic, etc.), adding rate-limiting, caching, cost tracking,
//! and request logging.
//!
//! API endpoints used:
//! - Proxy:  `POST https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}/{provider}/chat/completions`
//! - Logs:   `GET  https://api.cloudflare.com/client/v4/accounts/{account_id}/ai-gateway/gateways/{gateway_id}/logs`
//! - Analytics: `GET https://api.cloudflare.com/client/v4/accounts/{account_id}/ai-gateway/gateways/{gateway_id}/analytics`
//!
//! # Key dependencies
//! - [`clawz_core::error::ClawzError`] — unified error type.
//! - [`super::config::CloudflareConfig`] — provides account credentials and gateway ID.

use clawz_core::error::ClawzError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Dependency: pulls the top-level config to decide whether the service is enabled.
use super::config::CloudflareConfig;

// ── Public types ──────────────────────────────────────────────────────────────

/// Single entry from the AI Gateway request logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayLog {
    /// Unique log entry ID assigned by Cloudflare.
    pub id: String,
    /// Upstream provider slug, e.g. `"openai"` or `"anthropic"`.
    pub provider: String,
    /// Model name used for the request (if returned by the gateway).
    pub model: Option<String>,
    /// Whether the response was served from cache.
    pub cached: bool,
    /// HTTP status code returned by the provider.
    pub status_code: u16,
    /// End-to-end request duration in milliseconds.
    pub duration_ms: u64,
    /// Estimated cost in USD (if available from the gateway).
    pub cost: Option<f64>,
    /// ISO-8601 timestamp when the request was logged.
    pub created_at: String,
}

/// Aggregate analytics snapshot for an AI Gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayAnalytics {
    /// Total number of requests seen in the analytics window.
    pub total_requests: u64,
    /// Ratio of cache hits to total requests (0.0 – 1.0).
    pub cache_hit_rate: f64,
    /// Mean latency across all requests in the window, in milliseconds.
    pub avg_latency_ms: f64,
    /// Sum of estimated costs in USD for the analytics window.
    pub total_cost_usd: f64,
    /// Ratio of failed requests to total requests (0.0 – 1.0).
    pub error_rate: f64,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Client for the Cloudflare AI Gateway REST APIs.
pub struct AiGateway {
    /// Cloudflare account ID (from the dashboard).
    account_id: String,
    /// API token with `AI Gateway` read/write scopes.
    api_token: String,
    /// Gateway slug created in the Cloudflare dashboard.
    gateway_id: String,
    /// HTTP transport for all AI Gateway requests.
    client: reqwest::Client,
}

impl AiGateway {
    /// Construct from the master config; returns `None` when the service is
    /// disabled or the `gateway_id` is not configured.
    pub fn from_config(cfg: &CloudflareConfig) -> Option<Self> {
        // Both the global master switch and the per-service flag must be on.
        if !cfg.enabled || !cfg.ai_gateway.enabled {
            return None;
        }
        // `gateway_id` is optional in config; if missing we can't build a client.
        let gateway_id = cfg.ai_gateway.gateway_id.clone()?;
        Some(Self::new(
            cfg.account_id.clone(),
            cfg.api_token.clone(),
            gateway_id,
        ))
    }

    /// Direct constructor when credentials are already known.
    pub fn new(account_id: String, api_token: String, gateway_id: String) -> Self {
        Self {
            account_id,
            api_token,
            gateway_id,
            client: reqwest::Client::new(),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Base URL for the proxy endpoint (talks to `gateway.ai.cloudflare.com`).
    fn proxy_base(&self) -> String {
        format!(
            "https://gateway.ai.cloudflare.com/v1/{}/{}",
            self.account_id, self.gateway_id
        )
    }

    /// Base URL for the Cloudflare API v4 control plane.
    fn api_base(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/ai-gateway/gateways/{}",
            self.account_id, self.gateway_id
        )
    }

    /// Bearer token header value derived from the API token.
    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_token)
    }

    /// Parse the standard Cloudflare API envelope `{ success, errors, result }`.
    ///
    /// Returns the inner `result` value on success, or a [`ClawzError::Provider`]
    /// containing the concatenated error messages.
    fn unwrap_cf_response(json: Value) -> Result<Value, ClawzError> {
        let success = json.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if !success {
            let errors = json
                .get("errors")
                .and_then(|e| e.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|e| {
                            e.get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown error")
                                .to_owned()
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_else(|| "unknown error".into());
            return Err(ClawzError::Provider(format!("Cloudflare AI Gateway: {errors}")));
        }
        Ok(json.get("result").cloned().unwrap_or(Value::Null))
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Route an LLM request through the Cloudflare AI Gateway for a given
    /// provider (e.g. `"anthropic"`, `"openai"`).
    ///
    /// The `request` body must conform to the provider's own
    /// `/chat/completions` request schema.
    pub async fn proxy_request(&self, provider: &str, request: Value) -> Result<Value, ClawzError> {
        let url = format!("{}/{}/chat/completions", self.proxy_base(), provider);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("AI Gateway proxy request failed: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("AI Gateway response parse failed: {e}")))?;

        if !status.is_success() {
            // The proxy response is provider-formatted, not CF-envelope.
            let msg = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown provider error");
            if status.as_u16() == 429 {
                // Cloudflare signals rate-limiting via HTTP 429.
                return Err(ClawzError::RateLimited { retry_after_secs: 60 });
            }
            return Err(ClawzError::Provider(format!(
                "AI Gateway proxy returned {status}: {msg}"
            )));
        }

        Ok(body)
    }

    /// Fetch recent request logs from the gateway.
    ///
    /// `limit` caps the number of log entries returned (Cloudflare may impose
    /// its own max).
    pub async fn get_logs(&self, limit: usize) -> Result<Vec<GatewayLog>, ClawzError> {
        let url = format!("{}/logs?limit={}", self.api_base(), limit);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("AI Gateway logs request failed: {e}")))?;

        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("AI Gateway logs parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(body)?;

        // The CF API sometimes nests items under `items`; fall back to the raw result.
        let logs: Vec<GatewayLog> = serde_json::from_value(result.get("items").cloned().unwrap_or_else(|| result.clone()))
            .unwrap_or_default();

        Ok(logs)
    }

    /// Fetch aggregate analytics for the gateway (cost, latency, cache rate).
    pub async fn get_analytics(&self) -> Result<GatewayAnalytics, ClawzError> {
        let url = format!("{}/analytics", self.api_base());

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("AI Gateway analytics request failed: {e}")))?;

        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("AI Gateway analytics parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(body)?;

        // Map CF analytics fields to our struct, defaulting unknown fields to 0.
        // This protects against API changes that add or remove metrics.
        let analytics = GatewayAnalytics {
            total_requests: result
                .get("totalRequests")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_hit_rate: result
                .get("cacheHitRate")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            avg_latency_ms: result
                .get("avgLatencyMs")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            total_cost_usd: result
                .get("totalCostUsd")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            error_rate: result
                .get("errorRate")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        };

        Ok(analytics)
    }
}

// Keep old adapter name as an alias so existing code compiles unchanged.
pub type AiGatewayAdapter = AiGateway;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ai_gateway_from_disabled_config() {
        let cfg = CloudflareConfig::default();
        assert!(AiGateway::from_config(&cfg).is_none());
    }

    #[test]
    fn test_ai_gateway_from_enabled_config_missing_gateway_id() {
        let mut cfg = CloudflareConfig::default();
        cfg.enabled = true;
        cfg.ai_gateway.enabled = true;
        cfg.ai_gateway.gateway_id = None;
        assert!(AiGateway::from_config(&cfg).is_none());
    }

    #[test]
    fn test_ai_gateway_from_enabled_config() {
        let mut cfg = CloudflareConfig::default();
        cfg.enabled = true;
        cfg.account_id = "acc123".into();
        cfg.api_token = "tok456".into();
        cfg.ai_gateway.enabled = true;
        cfg.ai_gateway.gateway_id = Some("my-gw".into());
        let gw = AiGateway::from_config(&cfg).unwrap();
        assert_eq!(gw.gateway_id, "my-gw");
        assert_eq!(
            gw.proxy_base(),
            "https://gateway.ai.cloudflare.com/v1/acc123/my-gw"
        );
    }

    #[test]
    fn test_unwrap_cf_response_success() {
        let json = serde_json::json!({
            "success": true,
            "errors": [],
            "result": { "foo": "bar" }
        });
        let result = AiGateway::unwrap_cf_response(json).unwrap();
        assert_eq!(result["foo"], "bar");
    }

    #[test]
    fn test_unwrap_cf_response_error() {
        let json = serde_json::json!({
            "success": false,
            "errors": [{ "code": 1000, "message": "bad token" }],
            "result": null
        });
        let err = AiGateway::unwrap_cf_response(json).unwrap_err();
        assert!(err.to_string().contains("bad token"));
    }
}
