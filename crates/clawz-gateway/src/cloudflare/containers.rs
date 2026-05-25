//! Cloudflare Containers — container deployments on Cloudflare's edge.
//!
//! This module exposes an HTTP client for managing container workloads
//! (deploy, stop, logs, status) through the Cloudflare API.
//!
//! API base: `https://api.cloudflare.com/client/v4/accounts/{account_id}/containers`
//!
//! # Key dependencies
//! - [`clawz_core::error::ClawzError`] — unified error type.
//! - [`super::config::CloudflareConfig`] — provides account credentials.

use clawz_core::error::ClawzError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Dependency: top-level config used to decide if this client is active.
use super::config::CloudflareConfig;

// ── Public types ──────────────────────────────────────────────────────────────

/// Request payload for deploying a new container.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContainerConfig {
    /// Number of vCPUs to allocate.
    pub vcpu: Option<f32>,
    /// Memory in MiB.
    pub memory_mb: Option<u32>,
    /// Environment variables as key=value pairs.
    pub env: Option<Vec<(String, String)>>,
    /// Port to expose.
    pub port: Option<u16>,
    /// Command override (Docker CMD equivalent).
    pub command: Option<Vec<String>>,
}

/// Metadata returned by the Cloudflare Containers API for a single container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    /// Container UUID assigned by Cloudflare.
    pub id: String,
    /// Container image reference (e.g. `nginx:latest`).
    pub image: String,
    /// Current lifecycle state.
    pub status: ContainerStatus,
    /// ISO-8601 creation timestamp, if available.
    pub created_at: Option<String>,
    /// Assigned hostname for ingress, if any.
    pub hostname: Option<String>,
}

/// Lifecycle states a container can be in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus {
    /// Container has been requested but not yet started.
    Pending,
    /// Container is running and accepting traffic.
    Running,
    /// Container was stopped or exited cleanly.
    Stopped,
    /// Container failed to start or crashed.
    Failed,
    /// Status could not be determined from the API response.
    Unknown,
}

impl std::fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerStatus::Pending => write!(f, "pending"),
            ContainerStatus::Running => write!(f, "running"),
            ContainerStatus::Stopped => write!(f, "stopped"),
            ContainerStatus::Failed => write!(f, "failed"),
            ContainerStatus::Unknown => write!(f, "unknown"),
        }
    }
}

impl ContainerStatus {
    /// Parse a status string returned by the Cloudflare API.
    ///
    /// Accepts both the canonical names and CF-specific aliases
    /// (`"exited"` → `Stopped`, `"error"` → `Failed`).
    fn from_str(s: &str) -> Self {
        match s {
            "pending" => ContainerStatus::Pending,
            "running" => ContainerStatus::Running,
            "stopped" | "exited" => ContainerStatus::Stopped,
            "failed" | "error" => ContainerStatus::Failed,
            _ => ContainerStatus::Unknown,
        }
    }
}

// ── Client ────────────────────────────────────────────────────────────────────

/// HTTP client for the Cloudflare Containers API.
pub struct ContainersClient {
    /// Cloudflare account ID.
    account_id: String,
    /// API token with `Cloudflare Containers` scope.
    api_token: String,
    /// HTTP transport.
    client: reqwest::Client,
}

impl ContainersClient {
    /// Construct from the master config; returns `None` when the service is
    /// disabled.
    pub fn from_config(cfg: &CloudflareConfig) -> Option<Self> {
        if !cfg.enabled || !cfg.containers.enabled {
            return None;
        }
        Some(Self::new(cfg.account_id.clone(), cfg.api_token.clone()))
    }

    /// Direct constructor when credentials are already known.
    pub fn new(account_id: String, api_token: String) -> Self {
        Self {
            account_id,
            api_token,
            client: reqwest::Client::new(),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Root URL for the Containers API v4 endpoint.
    fn base_url(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/containers",
            self.account_id
        )
    }

    /// Bearer token header value.
    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_token)
    }

    /// Parse the standard Cloudflare API envelope.
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
            return Err(ClawzError::Provider(format!(
                "Cloudflare Containers error: {errors}"
            )));
        }
        Ok(json.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Convert a JSON value into a [`ContainerInfo`], filling in defaults for
    /// missing or malformed fields so that a partially-complete API response
    /// doesn't cause a hard failure.
    fn parse_container(v: &Value) -> ContainerInfo {
        ContainerInfo {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            image: v
                .get("image")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            status: ContainerStatus::from_str(
                v.get("status").and_then(|x| x.as_str()).unwrap_or("unknown"),
            ),
            created_at: v
                .get("created_at")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
            hostname: v
                .get("hostname")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Deploy a container image with the given configuration.
    ///
    /// Only fields that are `Some` are sent in the request body;
    /// omitted fields use Cloudflare's server-side defaults.
    pub async fn deploy(
        &self,
        image: &str,
        config: ContainerConfig,
    ) -> Result<ContainerInfo, ClawzError> {
        let url = self.base_url();

        let mut body = serde_json::json!({ "image": image });

        // Conditionally inject optional fields so we don't override defaults
        // with explicit nulls.
        if let Some(vcpu) = config.vcpu {
            body["vcpu"] = Value::from(vcpu);
        }
        if let Some(mem) = config.memory_mb {
            body["memory_mb"] = Value::from(mem);
        }
        if let Some(port) = config.port {
            body["port"] = Value::from(port);
        }
        if let Some(cmd) = config.command {
            body["command"] = Value::Array(cmd.into_iter().map(Value::String).collect());
        }
        if let Some(env) = config.env {
            let env_obj: serde_json::Map<String, Value> =
                env.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
            body["env"] = Value::Object(env_obj);
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers deploy failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers deploy parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(json)?;
        Ok(Self::parse_container(&result))
    }

    /// Get the current status of a container.
    pub async fn status(&self, container_id: &str) -> Result<ContainerStatus, ClawzError> {
        let url = format!("{}/{}", self.base_url(), container_id);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers status failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(ClawzError::NotFound {
                entity: "container".into(),
                id: container_id.to_owned(),
            });
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers status parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(json)?;
        let status_str = result
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        Ok(ContainerStatus::from_str(status_str))
    }

    /// Stop and remove a running container.
    pub async fn stop(&self, container_id: &str) -> Result<(), ClawzError> {
        let url = format!("{}/{}", self.base_url(), container_id);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers stop failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers stop parse failed: {e}")))?;

        Self::unwrap_cf_response(json)?;
        Ok(())
    }

    /// Stream or retrieve recent log lines for a container.
    ///
    /// The Cloudflare API may return logs either as a JSON envelope
    /// (`result: ["line1", "line2"]`) or as plain text.  We try JSON first
    /// and fall back to newline splitting.
    pub async fn logs(&self, container_id: &str) -> Result<Vec<String>, ClawzError> {
        let url = format!("{}/{}/logs", self.base_url(), container_id);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers logs failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "Containers logs returned {status}: {text}"
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| ClawzError::Transport(format!("Containers logs body read failed: {e}")))?;

        // Try to parse as JSON envelope first.
        if let Ok(json) = serde_json::from_str::<Value>(&body) {
            if let Ok(result) = Self::unwrap_cf_response(json) {
                let lines: Vec<String> = result
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok(lines);
            }
        }

        // Plain text fallback: split by newlines and drop empty fragments.
        let lines: Vec<String> = body
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_owned())
            .collect();

        Ok(lines)
    }
}

// Keep old adapter name as an alias so existing code compiles unchanged.
pub type ContainersAdapter = ContainersClient;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_containers_client_from_disabled_config() {
        let cfg = CloudflareConfig::default();
        assert!(ContainersClient::from_config(&cfg).is_none());
    }

    #[test]
    fn test_containers_client_from_enabled_config() {
        let mut cfg = CloudflareConfig::default();
        cfg.enabled = true;
        cfg.account_id = "acc123".into();
        cfg.api_token = "tok456".into();
        cfg.containers.enabled = true;
        let client = ContainersClient::from_config(&cfg).unwrap();
        assert_eq!(client.account_id, "acc123");
    }

    #[test]
    fn test_container_status_from_str() {
        assert_eq!(ContainerStatus::from_str("running"), ContainerStatus::Running);
        assert_eq!(ContainerStatus::from_str("exited"), ContainerStatus::Stopped);
        assert_eq!(ContainerStatus::from_str("error"), ContainerStatus::Failed);
        assert_eq!(ContainerStatus::from_str("blah"), ContainerStatus::Unknown);
    }

    #[test]
    fn test_parse_container_minimal() {
        let v = serde_json::json!({
            "id": "c1",
            "image": "nginx:latest",
            "status": "running"
        });
        let info = ContainersClient::parse_container(&v);
        assert_eq!(info.id, "c1");
        assert_eq!(info.status, ContainerStatus::Running);
    }

    #[test]
    fn test_logs_parse_plain_text() {
        // Simulate plain-text log response parsing.
        let plain = "line one\nline two\nline three\n";
        let lines: Vec<String> = plain
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_owned())
            .collect();
        assert_eq!(lines, vec!["line one", "line two", "line three"]);
    }
}
