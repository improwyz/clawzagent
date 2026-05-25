//! Deployment types — status, configuration, and deployment info.
//!
//! Used by deploy adapters (Docker, Fly, Railway, k8s, …) to describe
//! target infrastructure and report deployment state.
//!
//! // Dependency: used by worker::deploy adapters, gateway::deploy_handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ── DeployStatus ──────────────────────────────────────────────────────────────

/// Lifecycle states for a container / VM deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeployStatus {
    Pending,
    Building,
    Deploying,
    Running,
    Failed,
    Stopped,
}

impl std::fmt::Display for DeployStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployStatus::Pending => write!(f, "pending"),
            DeployStatus::Building => write!(f, "building"),
            DeployStatus::Deploying => write!(f, "deploying"),
            DeployStatus::Running => write!(f, "running"),
            DeployStatus::Failed => write!(f, "failed"),
            DeployStatus::Stopped => write!(f, "stopped"),
        }
    }
}

// ── ScalingConfig ─────────────────────────────────────────────────────────────

/// Auto-scaling parameters for elastic deployments.
/// // Dependency: embedded in DeployConfig, interpreted by worker::deploy adapters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingConfig {
    pub min_instances: u32,
    pub max_instances: u32,
    /// CPU utilisation percentage that triggers scale-out.
    pub scale_out_cpu_pct: u32,
    /// Cool-down period in seconds after a scale event.
    pub cooldown_secs: u32,
}

impl Default for ScalingConfig {
    fn default() -> Self {
        Self {
            min_instances: 1,
            max_instances: 4,
            scale_out_cpu_pct: 70,
            cooldown_secs: 60,
        }
    }
}

// ── DeployConfig ──────────────────────────────────────────────────────────────

/// Target infrastructure specification for a deployment.
/// // Dependency: passed to traits::DeployAdapter::deploy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    /// Deployment provider: "docker", "fly", "railway", "render", "k8s", etc.
    pub provider: String,
    pub region: String,
    pub instance_type: String,
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
    /// Names of secrets to inject (resolved by the deploy adapter from vault/env).
    #[serde(default)]
    pub secrets: Vec<String>,
    pub scaling: ScalingConfig,
    /// Optional OCI image tag to deploy. Overrides the default build image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Memory limit in MB.
    pub memory_mb: u32,
    pub cpu_millicores: u32,
}

impl DeployConfig {
    pub fn new(
        provider: impl Into<String>,
        region: impl Into<String>,
        instance_type: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            region: region.into(),
            instance_type: instance_type.into(),
            env_vars: HashMap::new(),
            secrets: Vec::new(),
            scaling: ScalingConfig::default(),
            image: None,
            memory_mb: 512,
            cpu_millicores: 250,
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }

    pub fn with_secret(mut self, secret_name: impl Into<String>) -> Self {
        self.secrets.push(secret_name.into());
        self
    }
}

// ── DeploymentInfo ────────────────────────────────────────────────────────────

/// Runtime snapshot of a deployment.
/// // Dependency: returned by traits::DeployAdapter, stored in db::DeploymentRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentInfo {
    pub id: String,
    pub provider: String,
    pub status: DeployStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub config: DeployConfig,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Cost per hour in USD (estimated by the adapter).
    pub cost_per_hour: f64,
}

impl DeploymentInfo {
    pub fn new(config: DeployConfig, provider: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            provider: provider.into(),
            status: DeployStatus::Pending,
            url: None,
            config,
            created_at: now,
            updated_at: now,
            cost_per_hour: 0.0,
        }
    }

    pub fn set_running(&mut self, url: impl Into<String>) {
        self.status = DeployStatus::Running;
        self.url = Some(url.into());
        self.updated_at = Utc::now();
    }

    pub fn set_failed(&mut self) {
        self.status = DeployStatus::Failed;
        self.updated_at = Utc::now();
    }

    pub fn set_stopped(&mut self) {
        self.status = DeployStatus::Stopped;
        self.updated_at = Utc::now();
    }
}
