//! Cloudflare service integrations for the ClawZ gateway.
//!
//! This module is the HTTP entry point for all Cloudflare-related operations.
//! It aggregates optional sub-services — AI Gateway, R2, KV, Tunnel, Containers,
//! and Workers — behind a single [`CloudflareManager`] that checks feature flags
//! before vending per-service clients.
//!
//! All services are OPTIONAL and individually toggled through
//! [`CloudflareConfig`].  When a service is disabled its accessor returns
//! `None`; callers must handle the absent case gracefully.
//!
//! # Key dependencies
//! - [`clawz_core::error::ClawzError`] — unified error type used by all clients.
//! - [`reqwest::Client`] — shared HTTP transport (currently one per client).

// Dependency: sub-service modules live in the same directory.
pub mod ai_gateway;
pub mod config;
pub mod containers;
pub mod kv;
pub mod r2;
pub mod tunnel;
pub mod workers;

// Re-export config types so downstream crates don't need deep paths.
pub use config::{
    CloudflareAiGatewayConfig, CloudflareConfig, CloudflareContainersConfig, CloudflareKvConfig,
    CloudflareR2Config, CloudflareTunnelConfig, CloudflareWorkersConfig,
};

// Re-export client types for the manager's accessor methods.
pub use ai_gateway::AiGateway;
pub use containers::ContainersClient;
pub use kv::KvClient;
pub use r2::R2Client;
pub use tunnel::TunnelClient;
pub use workers::WorkersClient;

// ── Manager ───────────────────────────────────────────────────────────────────

/// Top-level manager that owns the Cloudflare configuration and vends
/// per-service client handles.
///
/// Each accessor returns `None` when the corresponding service is disabled in
/// the configuration.  Clients are constructed on-demand (cheap — just a
/// config clone + shared `reqwest::Client`) so calling an accessor multiple
/// times is fine.
pub struct CloudflareManager {
    /// Parsed Cloudflare integration configuration (global + per-service flags).
    config: CloudflareConfig,
    // Reserved for future shared-client use (e.g. keep-alive connection pool).
    // We keep the field to avoid breaking the struct layout later.
    #[allow(dead_code)]
    client: reqwest::Client,
}

impl CloudflareManager {
    /// Create a new manager from the provided configuration.
    pub fn new(config: CloudflareConfig) -> Self {
        Self {
            config,
            // Currently unused — each sub-client builds its own reqwest client.
            client: reqwest::Client::new(),
        }
    }

    /// Return `true` when Cloudflare integrations are globally enabled *and*
    /// the named service is also enabled.
    ///
    /// Recognised service names: `"ai_gateway"`, `"r2"`, `"kv"`, `"tunnel"`,
    /// `"containers"`, `"workers"`.
    pub fn is_enabled(&self, service: &str) -> bool {
        // Fast path: master kill-switch disables everything.
        if !self.config.enabled {
            return false;
        }
        match service {
            "ai_gateway" => self.config.ai_gateway.enabled,
            "r2" => self.config.r2.enabled,
            "kv" => self.config.kv.enabled,
            "tunnel" => self.config.tunnel.enabled,
            "containers" => self.config.containers.enabled,
            "workers" => self.config.workers.enabled,
            _ => false,
        }
    }

    /// Get the AI Gateway client, if configured and enabled.
    // Dependency: ai_gateway::AiGateway::from_config
    pub fn ai_gateway(&self) -> Option<AiGateway> {
        AiGateway::from_config(&self.config)
    }

    /// Get the R2 object-storage client, if configured and enabled.
    // Dependency: r2::R2Client::from_config
    pub fn r2(&self) -> Option<R2Client> {
        R2Client::from_config(&self.config)
    }

    /// Get the Workers KV client, if configured and enabled.
    // Dependency: kv::KvClient::from_config
    pub fn kv(&self) -> Option<KvClient> {
        KvClient::from_config(&self.config)
    }

    /// Get the Tunnel client, if configured and enabled.
    // Dependency: tunnel::TunnelClient::from_config
    pub fn tunnel(&self) -> Option<TunnelClient> {
        TunnelClient::from_config(&self.config)
    }

    /// Get the Containers client, if configured and enabled.
    // Dependency: containers::ContainersClient::from_config
    pub fn containers(&self) -> Option<ContainersClient> {
        ContainersClient::from_config(&self.config)
    }

    /// Get the Workers client, if configured and enabled.
    // Dependency: workers::WorkersClient::from_config
    pub fn workers(&self) -> Option<WorkersClient> {
        WorkersClient::from_config(&self.config)
    }

    /// Expose the underlying config (read-only).
    pub fn config(&self) -> &CloudflareConfig {
        &self.config
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a config with every Cloudflare service explicitly enabled.
    /// Used across multiple tests to avoid repetition.
    fn fully_enabled_config() -> CloudflareConfig {
        CloudflareConfig {
            enabled: true,
            account_id: "acc".into(),
            api_token: "tok".into(),
            ai_gateway: CloudflareAiGatewayConfig {
                enabled: true,
                gateway_id: Some("gw".into()),
            },
            r2: CloudflareR2Config {
                enabled: true,
                bucket_name: Some("bkt".into()),
            },
            kv: CloudflareKvConfig {
                enabled: true,
                namespace_id: Some("ns".into()),
            },
            tunnel: CloudflareTunnelConfig {
                enabled: true,
                tunnel_id: None,
            },
            containers: CloudflareContainersConfig { enabled: true },
            workers: CloudflareWorkersConfig { enabled: true },
        }
    }

    #[test]
    fn test_manager_all_disabled_by_default() {
        // Default config has every flag set to false.
        let mgr = CloudflareManager::new(CloudflareConfig::default());
        assert!(!mgr.is_enabled("ai_gateway"));
        assert!(!mgr.is_enabled("r2"));
        assert!(!mgr.is_enabled("kv"));
        assert!(!mgr.is_enabled("tunnel"));
        assert!(!mgr.is_enabled("containers"));
        assert!(!mgr.is_enabled("workers"));
        assert!(mgr.ai_gateway().is_none());
        assert!(mgr.r2().is_none());
        assert!(mgr.kv().is_none());
        assert!(mgr.tunnel().is_none());
        assert!(mgr.containers().is_none());
        assert!(mgr.workers().is_none());
    }

    #[test]
    fn test_manager_all_enabled() {
        let mgr = CloudflareManager::new(fully_enabled_config());
        assert!(mgr.is_enabled("ai_gateway"));
        assert!(mgr.is_enabled("r2"));
        assert!(mgr.is_enabled("kv"));
        assert!(mgr.is_enabled("tunnel"));
        assert!(mgr.is_enabled("containers"));
        assert!(mgr.is_enabled("workers"));
        assert!(mgr.ai_gateway().is_some());
        assert!(mgr.r2().is_some());
        assert!(mgr.kv().is_some());
        assert!(mgr.tunnel().is_some());
        assert!(mgr.containers().is_some());
        assert!(mgr.workers().is_some());
    }

    #[test]
    fn test_manager_master_switch_overrides_service() {
        let mut cfg = fully_enabled_config();
        cfg.enabled = false; // master kill-switch
        let mgr = CloudflareManager::new(cfg);
        // Even though every sub-service is enabled, the master flag wins.
        assert!(!mgr.is_enabled("ai_gateway"));
        assert!(!mgr.is_enabled("r2"));
        assert!(mgr.ai_gateway().is_none());
    }

    #[test]
    fn test_manager_unknown_service_returns_false() {
        let mgr = CloudflareManager::new(fully_enabled_config());
        assert!(!mgr.is_enabled("nonexistent_service"));
    }

    #[test]
    fn test_manager_selective_disable() {
        let mut cfg = fully_enabled_config();
        cfg.kv.enabled = false;
        cfg.workers.enabled = false;
        let mgr = CloudflareManager::new(cfg);
        assert!(mgr.is_enabled("r2"));
        assert!(!mgr.is_enabled("kv"));
        assert!(!mgr.is_enabled("workers"));
        assert!(mgr.kv().is_none());
        assert!(mgr.workers().is_none());
    }
}
