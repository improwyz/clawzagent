//! Cloudflare Tunnel — secure inbound tunnels exposing local services.
//!
//! This module manages Cloudflare Tunnels (formerly "Argo Tunnel") through
//! the Cloudflare API v4.  It supports creating tunnels, listing them,
//! fetching their tokens, configuring ingress routes, and deleting them.
//!
//! API base: `https://api.cloudflare.com/client/v4/accounts/{account_id}/cfd_tunnel`
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

/// Metadata for a single Cloudflare Tunnel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelInfo {
    /// Tunnel UUID.
    pub id: String,
    /// Human-readable name given at creation time.
    pub name: String,
    /// Current tunnel state (e.g. `"healthy"`, `"down"`).
    pub status: String,
    /// ISO-8601 creation timestamp, if available.
    pub created_at: Option<String>,
    /// ISO-8601 deletion timestamp (non-null when soft-deleted).
    pub deleted_at: Option<String>,
    /// Account tag / identifier echoed by the API.
    pub account_tag: Option<String>,
    /// Tunnel type classification, if returned.
    pub tun_type: Option<String>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// HTTP client for the Cloudflare Tunnel (CFD) API.
pub struct TunnelClient {
    /// Cloudflare account ID.
    account_id: String,
    /// API token with `Cloudflare Tunnel` edit scope.
    api_token: String,
    /// HTTP transport.
    client: reqwest::Client,
}

impl TunnelClient {
    /// Construct from the master config; returns `None` when the service is
    /// disabled.
    pub fn from_config(cfg: &CloudflareConfig) -> Option<Self> {
        if !cfg.enabled || !cfg.tunnel.enabled {
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

    /// Root URL for the CFD Tunnel API v4 endpoint.
    fn base_url(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/cfd_tunnel",
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
                "Cloudflare Tunnel error: {errors}"
            )));
        }
        Ok(json.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Convert a JSON value into a [`TunnelInfo`], defaulting missing fields
    /// to empty strings / `None` so that partial API responses don't panic.
    fn parse_tunnel(v: &Value) -> TunnelInfo {
        TunnelInfo {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            status: v
                .get("status")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_owned(),
            created_at: v
                .get("created_at")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
            deleted_at: v
                .get("deleted_at")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
            account_tag: v
                .get("account_tag")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
            tun_type: v
                .get("tun_type")
                .and_then(|x| x.as_str())
                .map(|s| s.to_owned()),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Create a new Cloudflare Tunnel with the given name.
    ///
    /// A fresh UUID is generated automatically for the `tunnel_secret` field
    /// required by the create endpoint.
    pub async fn create_tunnel(&self, name: &str) -> Result<TunnelInfo, ClawzError> {
        let url = self.base_url();
        let body = serde_json::json!({ "name": name, "tunnel_secret": uuid::Uuid::new_v4().to_string() });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel create failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel create parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(json)?;
        Ok(Self::parse_tunnel(&result))
    }

    /// List all tunnels for the account (excludes deleted ones by default).
    pub async fn list_tunnels(&self) -> Result<Vec<TunnelInfo>, ClawzError> {
        // `is_deleted=false` hides soft-deleted tunnels from the listing.
        let url = format!("{}?is_deleted=false", self.base_url());

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel list failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel list parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(json)?;
        let tunnels = result
            .as_array()
            .map(|arr| arr.iter().map(Self::parse_tunnel).collect())
            .unwrap_or_default();
        Ok(tunnels)
    }

    /// Delete (clean up) a tunnel by ID.
    pub async fn delete_tunnel(&self, tunnel_id: &str) -> Result<(), ClawzError> {
        let url = format!("{}/{}", self.base_url(), tunnel_id);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel delete failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel delete parse failed: {e}")))?;

        Self::unwrap_cf_response(json)?;
        Ok(())
    }

    /// Retrieve the tunnel token (used to run `cloudflared tunnel run`).
    ///
    /// The token is returned as a plain string inside the `result` field.
    pub async fn get_tunnel_token(&self, tunnel_id: &str) -> Result<String, ClawzError> {
        let url = format!("{}/{}/token", self.base_url(), tunnel_id);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel get_token failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel get_token parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(json)?;

        // The token is a plain string in the result field.
        let token = result
            .as_str()
            .ok_or_else(|| ClawzError::Provider("Tunnel token is not a string".into()))?
            .to_owned();

        Ok(token)
    }

    /// Configure a hostname route for a tunnel (DNS ingress rule).
    ///
    /// This creates/updates the tunnel's ingress configuration to forward
    /// `hostname` traffic to `service_url` (e.g. `http://localhost:8080`).
    /// A catch-all `http_status:404` rule is appended because Cloudflare
    /// requires at least one fallback service.
    pub async fn configure_route(
        &self,
        tunnel_id: &str,
        hostname: &str,
        service_url: &str,
    ) -> Result<(), ClawzError> {
        let url = format!("{}/{}/configurations", self.base_url(), tunnel_id);

        let body = serde_json::json!({
            "config": {
                "ingress": [
                    {
                        "hostname": hostname,
                        "service": service_url
                    },
                    {
                        // catch-all rule required by Cloudflare
                        "service": "http_status:404"
                    }
                ]
            }
        });

        let resp = self
            .client
            .put(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("Tunnel configure_route failed: {e}")))?;

        let json: Value = resp
            .json()
            .await
            .map_err(|e| {
                ClawzError::Transport(format!("Tunnel configure_route parse failed: {e}"))
            })?;

        Self::unwrap_cf_response(json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tunnel_client_from_disabled_config() {
        let cfg = CloudflareConfig::default();
        assert!(TunnelClient::from_config(&cfg).is_none());
    }

    #[test]
    fn test_tunnel_client_from_enabled_config() {
        let mut cfg = CloudflareConfig::default();
        cfg.enabled = true;
        cfg.account_id = "acc123".into();
        cfg.api_token = "tok456".into();
        cfg.tunnel.enabled = true;
        let client = TunnelClient::from_config(&cfg).unwrap();
        assert_eq!(client.account_id, "acc123");
    }

    #[test]
    fn test_base_url() {
        let client = TunnelClient::new("acc".into(), "tok".into());
        assert_eq!(
            client.base_url(),
            "https://api.cloudflare.com/client/v4/accounts/acc/cfd_tunnel"
        );
    }

    #[test]
    fn test_parse_tunnel_minimal() {
        let v = serde_json::json!({ "id": "t1", "name": "my-tunnel", "status": "healthy" });
        let t = TunnelClient::parse_tunnel(&v);
        assert_eq!(t.id, "t1");
        assert_eq!(t.name, "my-tunnel");
        assert_eq!(t.status, "healthy");
        assert!(t.created_at.is_none());
    }

    #[test]
    fn test_unwrap_cf_response_error() {
        let json = serde_json::json!({
            "success": false,
            "errors": [{ "code": 1001, "message": "tunnel not found" }]
        });
        let err = TunnelClient::unwrap_cf_response(json).unwrap_err();
        assert!(err.to_string().contains("tunnel not found"));
    }
}
