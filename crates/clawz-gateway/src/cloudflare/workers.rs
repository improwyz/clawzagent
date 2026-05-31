//! Cloudflare Workers — edge function deployment.
//!
//! This module wraps the Cloudflare Workers script API, supporting upload,
//! deletion, and listing of worker scripts.  It is used by the gateway to
//! deploy edge functions that handle HTTP traffic, cron triggers, or
//! Durable Object events.
//!
//! API base: `https://api.cloudflare.com/client/v4/accounts/{account_id}/workers/scripts/{name}`
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

/// Metadata for a single deployed Worker script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    /// Worker script name / identifier.
    pub id: String,
    /// ETag returned by the upload response (content fingerprint).
    pub etag: Option<String>,
    /// Script size in bytes.
    pub size: Option<u64>,
    /// ISO-8601 last-modified timestamp.
    pub modified_on: Option<String>,
    /// ISO-8601 creation timestamp.
    pub created_on: Option<String>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// HTTP client for the Cloudflare Workers script API.
pub struct WorkersClient {
    /// Cloudflare account ID.
    account_id: String,
    /// API token with `Cloudflare Workers` edit scope.
    api_token: String,
    /// HTTP transport.
    client: reqwest::Client,
}

impl WorkersClient {
    /// Construct from the master config; returns `None` when the service is
    /// disabled.
    pub fn from_config(cfg: &CloudflareConfig) -> Option<Self> {
        if !cfg.enabled || !cfg.workers.enabled {
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

    /// URL for a specific script by name.
    fn scripts_url(&self, name: &str) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/workers/scripts/{}",
            self.account_id, name
        )
    }

    /// URL for listing all scripts in the account.
    fn list_url(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/workers/scripts",
            self.account_id
        )
    }

    /// Bearer token header value.
    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_token)
    }

    /// Parse the standard Cloudflare API envelope.
    fn unwrap_cf_response(json: Value) -> Result<Value, ClawzError> {
        let success = json
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
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
                "Cloudflare Workers error: {errors}"
            )));
        }
        Ok(json.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Convert a JSON value into a [`WorkerInfo`], defaulting missing fields.
    fn parse_worker(v: &Value) -> WorkerInfo {
        WorkerInfo {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            etag: v.get("etag").and_then(|x| x.as_str()).map(|s| s.to_owned()),
            size: v.get("size").and_then(|x| x.as_u64()),
            modified_on: v
                .get("modified_on")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
            created_on: v
                .get("created_on")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Upload/update a Worker script.
    ///
    /// The `script` parameter should be valid JavaScript (or TypeScript if your
    /// account has the Workers for Platforms feature).  The script is uploaded
    /// as a `multipart/form-data` body built manually so that no extra reqwest
    /// feature flags are required.
    pub async fn deploy_worker(&self, name: &str, script: &str) -> Result<WorkerInfo, ClawzError> {
        let url = self.scripts_url(name);

        // Build a minimal multipart/form-data body manually.
        // We avoid reqwest's multipart feature to keep the dependency tree slim.
        let boundary = format!("boundary-{}", uuid::Uuid::new_v4());
        let metadata = r#"{"main_module":"worker.js","compatibility_date":"2024-01-01"}"#;

        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"worker.js\"; filename=\"worker.js\"\r\nContent-Type: application/javascript\r\n\r\n{script}\r\n\
             --{boundary}--\r\n",
        );

        let content_type = format!("multipart/form-data; boundary={boundary}");

        let resp = self
            .client
            .put(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Type", content_type)
            .body(body)
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Workers deploy failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Workers deploy parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(json)?;
        Ok(Self::parse_worker(&result))
    }

    /// Delete a Worker script by name.
    pub async fn delete_worker(&self, name: &str) -> Result<(), ClawzError> {
        let url = self.scripts_url(name);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Workers delete failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(ClawzError::NotFound {
                entity: "worker".into(),
                id: name.to_owned(),
            });
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Workers delete parse failed: {e}")))?;

        Self::unwrap_cf_response(json)?;
        Ok(())
    }

    /// List all Worker scripts in the account.
    pub async fn list_workers(&self) -> Result<Vec<WorkerInfo>, ClawzError> {
        let url = self.list_url();

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Workers list failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Workers list parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(json)?;
        let workers = result
            .as_array()
            .map(|arr| arr.iter().map(Self::parse_worker).collect())
            .unwrap_or_default();
        Ok(workers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workers_client_from_disabled_config() {
        let cfg = CloudflareConfig::default();
        assert!(WorkersClient::from_config(&cfg).is_none());
    }

    #[test]
    fn test_workers_client_from_enabled_config() {
        let mut cfg = CloudflareConfig::default();
        cfg.enabled = true;
        cfg.account_id = "acc123".into();
        cfg.api_token = "tok456".into();
        cfg.workers.enabled = true;
        let client = WorkersClient::from_config(&cfg).unwrap();
        assert_eq!(client.account_id, "acc123");
    }

    #[test]
    fn test_scripts_url() {
        let client = WorkersClient::new("acc".into(), "tok".into());
        assert_eq!(
            client.scripts_url("my-worker"),
            "https://api.cloudflare.com/client/v4/accounts/acc/workers/scripts/my-worker"
        );
    }

    #[test]
    fn test_parse_worker_full() {
        let v = serde_json::json!({
            "id": "my-worker",
            "etag": "abc123",
            "size": 4096,
            "created_on": "2026-01-01T00:00:00Z",
            "modified_on": "2026-01-02T00:00:00Z"
        });
        let w = WorkersClient::parse_worker(&v);
        assert_eq!(w.id, "my-worker");
        assert_eq!(w.etag, Some("abc123".into()));
        assert_eq!(w.size, Some(4096));
    }

    #[test]
    fn test_unwrap_cf_response_success() {
        let json = serde_json::json!({
            "success": true,
            "errors": [],
            "result": [{ "id": "w1" }]
        });
        let result = WorkersClient::unwrap_cf_response(json).unwrap();
        assert!(result.is_array());
    }
}
