//! Configuration system for the ClawZ platform.
//!
//! `AppConfig` is the central configuration tree loaded from TOML and
//! overridden by environment variables. It is consumed by both the
//! `clawz-worker` runtime and the `clawz-gateway` API server.
//!
//! // Dependency: consumed by worker::scheduler, gateway::server

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{ClawzError, Result};

// ── Server ────────────────────────────────────────────────────────────────────

/// HTTP server binding and worker-pool settings.
/// // Dependency: used by gateway::server to bind the axum/Actix listener.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Bind address (e.g. "0.0.0.0").
    /// // Env override: CLAWZ_SERVER_HOST
    pub host: String,
    /// TCP port to listen on.
    /// // Env override: CLAWZ_SERVER_PORT
    pub port: u16,
    /// Number of OS threads for the async runtime.
    /// Defaults to available parallelism.
    /// // Env override: CLAWZ_SERVER_WORKERS
    pub workers: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            workers: num_cpus(),
        }
    }
}

// Use available parallelism so we don't over/under-subscribe the CPU.
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

// ── Database ──────────────────────────────────────────────────────────────────

/// PostgreSQL connection-pool settings.
/// // Dependency: used by db::run_migrations and all repository helpers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Full libpq connection string.
    /// // Env override: CLAWZ_DATABASE_URL
    pub url: String,
    /// Max connections in the sqlx pool.
    /// // Env override: CLAWZ_DATABASE_MAX_CONNECTIONS
    pub max_connections: u32,
    /// Min idle connections to keep warm.
    pub min_connections: u32,
    /// Connection acquisition timeout in seconds.
    pub acquire_timeout_secs: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://localhost/clawz".to_string(),
            max_connections: 20,
            min_connections: 2,
            acquire_timeout_secs: 30,
        }
    }
}

// ── ProviderConfig ────────────────────────────────────────────────────────────

/// Single LLM provider entry (OpenAI, Anthropic, Gemini, Mistral, …).
/// // Dependency: used by worker::provider_router to select and rate-limit backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Human-readable name, also used as lookup key.
    pub name: String,
    /// Environment variable that holds the API key.
    pub api_key_env: String,
    /// Custom base URL for self-hosted or proxy endpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// List of models exposed by this provider.
    #[serde(default)]
    pub models: Vec<String>,
    /// Model to use when none is specified.
    pub default_model: String,
    /// Requests per minute limit (0 = no limit).
    pub rate_limit_rpm: u32,
}

impl ProviderConfig {
    /// Read the API key from the environment variable named in `api_key_env`.
    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.api_key_env).ok()
    }

    /// Returns true if the API key env var is present and non-empty.
    pub fn is_configured(&self) -> bool {
        self.api_key().is_some()
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: "anthropic".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: None,
            models: vec!["claude-sonnet-4-5".to_string()],
            default_model: "claude-sonnet-4-5".to_string(),
            rate_limit_rpm: 0,
        }
    }
}

// ── Mesh ──────────────────────────────────────────────────────────────────────

/// WireGuard / overlay-network settings for peer-to-peer agent mesh.
/// // Dependency: used by worker::mesh and traits::TenantMesh impls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshConfig {
    /// Whether to join the overlay network on startup.
    /// // Env override: CLAWZ_MESH_ENABLED
    pub enabled: bool,
    /// Network name passed to the mesh controller (e.g. NetBird).
    pub network_name: String,
    /// UDP port for WireGuard listen.
    pub listen_port: u16,
    /// Management API endpoint (e.g. NetBird or Tailscale).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_url: Option<String>,
    /// Environment variable holding the mesh auth key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_key_env: Option<String>,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            network_name: "clawz".to_string(),
            listen_port: 51820,
            management_url: None,
            auth_key_env: None,
        }
    }
}

// ── Governance ────────────────────────────────────────────────────────────────

/// Policy engine and trust-model settings.
/// // Dependency: used by worker::governance_engine and gateway::auth middleware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceConfig {
    /// Master switch — when false all actions are allowed.
    /// // Env override: CLAWZ_GOVERNANCE_ENABLED
    pub enabled: bool,
    /// Name of the default policy set to load on startup.
    pub default_policy: String,
    /// How much trust decays per hour (0.0 – 1.0).
    pub trust_decay_rate: f64,
    /// Require human approval for all `Deny` outcomes.
    pub require_approval_on_deny: bool,
}

impl Default for GovernanceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_policy: "standard".to_string(),
            trust_decay_rate: 0.01,
            require_approval_on_deny: false,
        }
    }
}

// ── Deploy ────────────────────────────────────────────────────────────────────

/// Default deployment adapter settings (Docker, Fly, Railway, k8s, …).
/// // Dependency: used by worker::deploy adapters and gateway::deploy API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployAppConfig {
    /// Default provider name when none is specified in a deploy request.
    pub default_provider: String,
    /// OCI registry for agent/tool images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
    /// Default OCI registry credentials environment variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_auth_env: Option<String>,
}

impl Default for DeployAppConfig {
    fn default() -> Self {
        Self {
            default_provider: "docker".to_string(),
            registry_url: None,
            registry_auth_env: None,
        }
    }
}

// ── Telemetry ─────────────────────────────────────────────────────────────────

/// OpenTelemetry / logging configuration.
/// // Dependency: initialised by both worker and gateway on startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether to export traces and metrics.
    pub enabled: bool,
    /// OTLP endpoint (e.g. http://otel-collector:4317)
    /// // Env override: CLAWZ_OTEL_ENDPOINT
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Fraction of traces to sample (0.0 – 1.0).
    pub sample_rate: f64,
    /// Rust/tracing log level string.
    /// // Env override: CLAWZ_LOG_LEVEL
    pub log_level: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endpoint: None,
            sample_rate: 0.1,
            log_level: "info".to_string(),
        }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

/// Root configuration aggregate — deserialized from `clawz.toml` or built from
/// defaults and env overrides.
/// // Dependency: worker::bootstrap, gateway::bootstrap both call `AppConfig::load()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    pub mesh: MeshConfig,
    pub governance: GovernanceConfig,
    pub deploy: DeployAppConfig,
    pub telemetry: TelemetryConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            providers: vec![ProviderConfig::default()],
            mesh: MeshConfig::default(),
            governance: GovernanceConfig::default(),
            deploy: DeployAppConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load from a TOML file at `path`.
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| ClawzError::Config(format!("cannot read {}: {e}", path.display())))?;
        let mut cfg: AppConfig = toml::from_str(&raw)?;
        cfg.apply_env_overrides();
        Ok(cfg)
    }

    /// Load from the path given by the `CLAWZ_CONFIG` env var, falling back to
    /// `clawz.toml` in the current directory, and finally to defaults.
    pub fn load() -> Result<Self> {
        let path = std::env::var("CLAWZ_CONFIG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("clawz.toml"));

        if path.exists() {
            Self::from_file(&path)
        } else {
            let mut cfg = AppConfig::default();
            cfg.apply_env_overrides();
            Ok(cfg)
        }
    }

    /// Apply environment-variable overrides following the pattern
    /// `CLAWZ_<SECTION>_<FIELD>` (e.g. `CLAWZ_SERVER_PORT`).
    /// This allows containerised deployments to tweak settings without
    /// mounting a new TOML file.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("CLAWZ_SERVER_PORT") {
            if let Ok(port) = v.parse::<u16>() {
                self.server.port = port;
            }
        }
        if let Ok(v) = std::env::var("CLAWZ_SERVER_HOST") {
            self.server.host = v;
        }
        if let Ok(v) = std::env::var("CLAWZ_SERVER_WORKERS") {
            if let Ok(w) = v.parse::<usize>() {
                self.server.workers = w;
            }
        }
        if let Ok(v) = std::env::var("CLAWZ_DATABASE_URL") {
            self.database.url = v;
        }
        if let Ok(v) = std::env::var("CLAWZ_DATABASE_MAX_CONNECTIONS") {
            if let Ok(n) = v.parse::<u32>() {
                self.database.max_connections = n;
            }
        }
        if let Ok(v) = std::env::var("CLAWZ_MESH_ENABLED") {
            self.mesh.enabled = matches!(v.to_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("CLAWZ_GOVERNANCE_ENABLED") {
            self.governance.enabled = matches!(v.to_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("CLAWZ_LOG_LEVEL") {
            self.telemetry.log_level = v;
        }
        if let Ok(v) = std::env::var("CLAWZ_OTEL_ENDPOINT") {
            self.telemetry.endpoint = Some(v);
        }
    }

    /// Serialise the current config back to a TOML string.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(ClawzError::from)
    }

    /// Find a `ProviderConfig` by name.
    pub fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name)
    }
}
