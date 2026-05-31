//! Cloudflare Workers KV — key-value storage for config and runtime state.
//!
//! This module wraps the Cloudflare Workers KV REST API, exposing basic
//! CRUD operations (`get`, `put`, `delete`, `list`) with optional TTL.
//!
//! API base: `https://api.cloudflare.com/client/v4/accounts/{account_id}/storage/kv/namespaces/{namespace_id}`
//!
//! # Key dependencies
//! - [`clawz_core::error::ClawzError`] — unified error type.
//! - [`super::config::CloudflareConfig`] — provides account credentials and namespace ID.

use clawz_core::error::ClawzError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Dependency: top-level config used to decide if this client is active.
use super::config::CloudflareConfig;

// ── Public types ──────────────────────────────────────────────────────────────

/// Metadata for a single KV key returned by the list endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KvKey {
    /// The key name (may include path-like prefixes).
    pub name: String,
    /// Unix timestamp when the key will expire, if a TTL was set.
    pub expiration: Option<u64>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// HTTP client for the Cloudflare Workers KV API.
pub struct KvClient {
    /// Cloudflare account ID.
    account_id: String,
    /// API token with `Workers KV` read/write scopes.
    api_token: String,
    /// KV namespace ID.
    namespace_id: String,
    /// HTTP transport.
    client: reqwest::Client,
}

impl KvClient {
    /// Construct from the master config; returns `None` when the service is
    /// disabled or the `namespace_id` is not configured.
    pub fn from_config(cfg: &CloudflareConfig) -> Option<Self> {
        if !cfg.enabled || !cfg.kv.enabled {
            return None;
        }
        let namespace_id = cfg.kv.namespace_id.clone()?;
        Some(Self::new(
            cfg.account_id.clone(),
            cfg.api_token.clone(),
            namespace_id,
        ))
    }

    /// Direct constructor when credentials and namespace are already known.
    pub fn new(account_id: String, api_token: String, namespace_id: String) -> Self {
        Self {
            account_id,
            api_token,
            namespace_id,
            client: reqwest::Client::new(),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Root URL for this namespace.
    fn ns_url(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/storage/kv/namespaces/{}",
            self.account_id, self.namespace_id
        )
    }

    /// URL for a specific key's value endpoint.
    ///
    /// Keys are percent-encoded so that slashes, spaces, and unicode characters
    /// do not break the URL path.
    fn value_url(&self, key: &str) -> String {
        // URL-encode the key so that slashes and spaces are safe.
        let encoded: String = key
            .chars()
            .flat_map(|c| {
                // Keep unreserved RFC 3986 characters as-is.
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    vec![c]
                } else {
                    // Percent-encode each byte of the UTF-8 representation.
                    c.to_string()
                        .bytes()
                        .flat_map(|b| format!("%{b:02X}").chars().collect::<Vec<_>>())
                        .collect()
                }
            })
            .collect();
        format!("{}/values/{}", self.ns_url(), encoded)
    }

    /// URL for listing keys, optionally filtered by prefix.
    fn keys_url(&self, prefix: Option<&str>) -> String {
        match prefix {
            Some(p) if !p.is_empty() => {
                format!("{}/keys?prefix={}", self.ns_url(), p)
            }
            _ => format!("{}/keys", self.ns_url()),
        }
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
                "Cloudflare KV error: {errors}"
            )));
        }
        Ok(json.get("result").cloned().unwrap_or(Value::Null))
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Retrieve a value by key; returns `None` when the key does not exist.
    pub async fn get(&self, key: &str) -> Result<Option<String>, ClawzError> {
        let url = self.value_url(key);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("KV get failed: {e}")))?;

        // KV returns 404 for missing keys rather than an error envelope.
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "KV get returned {status}: {text}"
            )));
        }

        let value = resp
            .text()
            .await
            .map_err(|e| ClawzError::Transport(format!("KV get body read failed: {e}")))?;

        Ok(Some(value))
    }

    /// Store a value; optionally provide a TTL in seconds.
    ///
    /// The TTL is passed as `expiration_ttl` on the query string, which tells
    /// Cloudflare to auto-expire the key after the given duration.
    pub async fn put(&self, key: &str, value: &str, ttl: Option<u64>) -> Result<(), ClawzError> {
        let mut url = self.value_url(key);
        if let Some(t) = ttl {
            url.push_str(&format!("?expiration_ttl={t}"));
        }

        let resp = self
            .client
            .put(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Type", "text/plain")
            .body(value.to_owned())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("KV put failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "KV put returned {status}: {text}"
            )));
        }

        Ok(())
    }

    /// Delete a key from the namespace.
    pub async fn delete(&self, key: &str) -> Result<(), ClawzError> {
        let url = self.value_url(key);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("KV delete failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "KV delete returned {status}: {text}"
            )));
        }

        Ok(())
    }

    /// List keys in the namespace, optionally filtered by prefix.
    pub async fn list(&self, prefix: Option<&str>) -> Result<Vec<KvKey>, ClawzError> {
        let url = self.keys_url(prefix);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("KV list failed: {e}")))?;

        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Transport(format!("KV list parse failed: {e}")))?;

        let result = Self::unwrap_cf_response(body)?;

        let keys: Vec<KvKey> = serde_json::from_value(result).unwrap_or_default();
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kv_client_from_disabled_config() {
        let cfg = CloudflareConfig::default();
        assert!(KvClient::from_config(&cfg).is_none());
    }

    #[test]
    fn test_kv_client_from_enabled_config() {
        let mut cfg = CloudflareConfig {
            enabled: true,
            account_id: "acc123".into(),
            api_token: "tok456".into(),
            ..Default::default()
        };
        cfg.kv.enabled = true;
        cfg.kv.namespace_id = Some("ns-abc".into());
        let client = KvClient::from_config(&cfg).unwrap();
        assert_eq!(client.namespace_id, "ns-abc");
    }

    #[test]
    fn test_value_url_simple_key() {
        let client = KvClient::new("acc".into(), "tok".into(), "ns".into());
        let url = client.value_url("my-key");
        assert!(url.ends_with("/values/my-key"));
    }

    #[test]
    fn test_keys_url_with_prefix() {
        let client = KvClient::new("acc".into(), "tok".into(), "ns".into());
        let url = client.keys_url(Some("cfg/"));
        assert!(url.contains("prefix=cfg/"));
    }

    #[test]
    fn test_keys_url_without_prefix() {
        let client = KvClient::new("acc".into(), "tok".into(), "ns".into());
        let url = client.keys_url(None);
        assert!(url.ends_with("/keys"));
    }
}
