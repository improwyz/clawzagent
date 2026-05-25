//! Cloudflare integration configuration — deserialized from the gateway's
//! application config file (e.g. TOML / JSON).
//!
//! Every service is gated by an `enabled` flag so that operators can opt-in
//! to individual Cloudflare products without touching code.
//!
//! # Key dependencies
//! - `serde` — all structs derive `Serialize` + `Deserialize` for config loading.
//! - Consumed by [`super::CloudflareManager`] and every per-service client.

use serde::{Deserialize, Serialize};

/// Top-level Cloudflare integration configuration.
///
/// Holds the global master switch plus per-service sub-configs and the
/// shared account credentials used by every client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareConfig {
    /// Master kill-switch: when false, no Cloudflare service will be used
    /// regardless of individual sub-service flags.
    pub enabled: bool,
    /// Cloudflare account ID (found in the dashboard URL).
    pub account_id: String,
    /// API token with scopes matching the enabled services.
    pub api_token: String,
    /// AI Gateway sub-configuration.
    pub ai_gateway: CloudflareAiGatewayConfig,
    /// R2 object-storage sub-configuration.
    pub r2: CloudflareR2Config,
    /// Workers KV sub-configuration.
    pub kv: CloudflareKvConfig,
    /// Cloudflare Tunnel sub-configuration.
    pub tunnel: CloudflareTunnelConfig,
    /// Cloudflare Containers sub-configuration.
    pub containers: CloudflareContainersConfig,
    /// Cloudflare Workers sub-configuration.
    pub workers: CloudflareWorkersConfig,
}

/// Cloudflare AI Gateway configuration.
///
/// Controls whether LLM requests are proxied through Cloudflare's AI Gateway
/// for caching, logging, and rate-limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareAiGatewayConfig {
    /// Enable AI Gateway integration.
    pub enabled: bool,
    /// Gateway slug (e.g. `"my-gateway"`) as created in the CF dashboard.
    pub gateway_id: Option<String>,
}

/// Cloudflare R2 object-storage configuration.
///
/// R2 is an S3-compatible store used for persisting agent artifacts,
/// logs, and large binary payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareR2Config {
    /// Enable R2 integration.
    pub enabled: bool,
    /// Bucket name to store agent artifacts in.
    pub bucket_name: Option<String>,
}

/// Cloudflare Workers KV configuration.
///
/// KV provides low-latency key-value storage ideal for configuration,
/// routing tables, and lightweight runtime state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareKvConfig {
    /// Enable Workers KV integration.
    pub enabled: bool,
    /// KV namespace ID.
    pub namespace_id: Option<String>,
}

/// Cloudflare Tunnel configuration.
///
/// Tunnels expose local gateway services to the public internet through
/// Cloudflare's edge without opening firewall ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareTunnelConfig {
    /// Enable Cloudflare Tunnel integration.
    pub enabled: bool,
    /// Pre-existing tunnel ID (optional; a new tunnel is created if absent).
    pub tunnel_id: Option<String>,
}

/// Cloudflare Containers configuration.
///
/// Controls edge container deployments for running isolated workloads
/// close to users.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareContainersConfig {
    /// Enable Cloudflare Containers integration.
    pub enabled: bool,
}

/// Cloudflare Workers configuration.
///
/// Workers are edge functions that can handle HTTP traffic, cron triggers,
/// and Durable Object state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareWorkersConfig {
    /// Enable Cloudflare Workers integration.
    pub enabled: bool,
}

// ── Defaults ─────────────────────────────────────────────────────────────────

impl Default for CloudflareConfig {
    fn default() -> Self {
        Self {
            // Safe default: every integration starts disabled so that a missing
            // config file doesn't accidentally send traffic to Cloudflare.
            enabled: false,
            account_id: String::new(),
            api_token: String::new(),
            ai_gateway: CloudflareAiGatewayConfig::default(),
            r2: CloudflareR2Config::default(),
            kv: CloudflareKvConfig::default(),
            tunnel: CloudflareTunnelConfig::default(),
            containers: CloudflareContainersConfig::default(),
            workers: CloudflareWorkersConfig::default(),
        }
    }
}

impl Default for CloudflareAiGatewayConfig {
    fn default() -> Self {
        Self { enabled: false, gateway_id: None }
    }
}

impl Default for CloudflareR2Config {
    fn default() -> Self {
        Self { enabled: false, bucket_name: None }
    }
}

impl Default for CloudflareKvConfig {
    fn default() -> Self {
        Self { enabled: false, namespace_id: None }
    }
}

impl Default for CloudflareTunnelConfig {
    fn default() -> Self {
        Self { enabled: false, tunnel_id: None }
    }
}

impl Default for CloudflareContainersConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

impl Default for CloudflareWorkersConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}
