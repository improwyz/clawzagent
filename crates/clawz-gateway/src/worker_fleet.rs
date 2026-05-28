//! HTTP client for worker fleet orchestration (`/v1/fleet/*`).

use clawz_core::error::{ClawzError, Result};
use clawz_core::types::orchestration::{AgentHandle, AgentSpec};
use clawz_core::types::tenant::{Role, TenantContext};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Calls the worker control API to spawn and list tenant-scoped agent containers.
pub struct WorkerFleetClient {
    base_url: String,
    token: String,
    http: Client,
}

#[derive(Debug, Serialize)]
struct FleetSpawnRequest {
    tenant_id: String,
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    spec: Option<AgentSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capabilities: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct FleetSpawnResponse {
    handle: AgentHandle,
}

#[derive(Debug, Deserialize)]
struct FleetListResponse {
    agents: Vec<AgentHandle>,
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Operator => "operator",
        Role::Agent => "agent",
        Role::Tool => "tool",
        Role::Viewer => "viewer",
    }
}

impl WorkerFleetClient {
    pub fn from_env() -> Option<Self> {
        let base = std::env::var("CLAWZ_WORKER_FLEET_URL")
            .or_else(|_| std::env::var("WORKER_URL"))
            .ok()?;
        if base.is_empty() {
            return None;
        }
        Some(Self::new(base))
    }

    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let token = std::env::var("CLAWZ_WORKER_TOKEN")
            .or_else(|_| std::env::var("WORKER_INTERNAL_TOKEN"))
            .unwrap_or_default();
        Self {
            base_url,
            token,
            http: Client::new(),
        }
    }

    fn auth_header(&self) -> Option<String> {
        if self.token.is_empty() {
            None
        } else {
            Some(format!("Bearer {}", self.token))
        }
    }

    pub async fn spawn(
        &self,
        ctx: &TenantContext,
        spec: AgentSpec,
        capabilities: &[String],
    ) -> Result<AgentHandle> {
        let body = FleetSpawnRequest {
            tenant_id: ctx.tenant_id.to_string(),
            role: role_to_str(ctx.role).to_string(),
            spec: Some(spec),
            capabilities: if capabilities.is_empty() {
                None
            } else {
                Some(capabilities.to_vec())
            },
        };

        let mut req = self
            .http
            .post(format!("{}/v1/fleet/spawn", self.base_url))
            .json(&body);
        if let Some(h) = self.auth_header() {
            req = req.header("Authorization", h);
        }

        let resp = req.send().await.map_err(|e| {
            ClawzError::Orchestration(format!("worker fleet spawn request failed: {e}"))
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Orchestration(format!(
                "worker fleet spawn failed ({status}): {text}"
            )));
        }

        let parsed: FleetSpawnResponse = resp.json().await.map_err(|e| {
            ClawzError::Orchestration(format!("worker fleet spawn response parse failed: {e}"))
        })?;
        Ok(parsed.handle)
    }

    pub async fn list_agents(&self, tenant_id: &str) -> Result<Vec<AgentHandle>> {
        let mut req = self
            .http
            .get(format!("{}/v1/fleet/agents", self.base_url))
            .query(&[("tenant_id", tenant_id)]);
        if let Some(h) = self.auth_header() {
            req = req.header("Authorization", h);
        }

        let resp = req.send().await.map_err(|e| {
            ClawzError::Orchestration(format!("worker fleet list request failed: {e}"))
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Orchestration(format!(
                "worker fleet list failed ({status}): {text}"
            )));
        }

        let parsed: FleetListResponse = resp.json().await.map_err(|e| {
            ClawzError::Orchestration(format!("worker fleet list response parse failed: {e}"))
        })?;
        Ok(parsed.agents)
    }
}
