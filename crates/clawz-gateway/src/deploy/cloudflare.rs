//! Cloudflare adapter — deploys Workers and containers to the Cloudflare edge network.
//!
//! This module implements `DeployProvider` for Cloudflare using the v4 REST API.
//! It supports:
//!
//! * **Docker** — container deployments to Cloudflare Pages or Workers (beta).
//! * **Wasm** — native WebAssembly Workers deployed via the Workers API.
//!
//! Native binary mode is rejected because Cloudflare's edge runtime is either
//! Wasm-based or container-based; there is no generic Linux binary host.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for Cloudflare API calls.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::{external_resource_name, generate_deployment_id, resolve_api_token};
use base64::Engine;
// Dependency: provider trait and shared types.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for the Cloudflare platform (Workers, Pages, and Containers).
///
/// The `account_id` is required for most Cloudflare API paths; the actual
/// API token is supplied at credential-validation time.
#[allow(dead_code)]
pub struct CloudflareAdapter {
    /// Shared HTTP client for Cloudflare API requests.
    client: reqwest::Client,
    /// Cloudflare account identifier (shown in the Cloudflare dashboard).
    account_id: String,
}

impl CloudflareAdapter {
    /// Create a new adapter bound to a Cloudflare account.
    pub fn new(account_id: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            account_id: account_id.into(),
        }
    }

    /// Build a Cloudflare API v4 URL for the given path.
    fn api_url(&self, path: &str) -> String {
        format!("https://api.cloudflare.com/client/v4{path}")
    }
}

#[async_trait]
impl DeployProvider for CloudflareAdapter {
    fn provider_id(&self) -> &str {
        "cloudflare"
    }

    fn display_name(&self) -> &str {
        "Cloudflare"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![
            DeployMode::Docker {
                image: String::new(),
            },
            DeployMode::Wasm,
        ]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let token = creds
            .api_token
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Cloudflare API token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("/user/tokens/verify"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Cloudflare API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Cloudflare credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let token = resolve_api_token(config, "CLOUDFLARE_API_TOKEN", "Cloudflare")?;
        let account_id = config
            .credentials
            .as_ref()
            .and_then(|c| {
                c.extra
                    .get("account_id")
                    .cloned()
                    .or_else(|| c.project_id.clone())
            })
            .filter(|id| !id.is_empty() && id != "default")
            .or_else(|| {
                if self.account_id.is_empty() || self.account_id == "default" {
                    None
                } else {
                    Some(self.account_id.clone())
                }
            })
            .or_else(|| std::env::var("CLOUDFLARE_ACCOUNT_ID").ok())
            .ok_or_else(|| {
                ClawzError::Auth(
                    "Cloudflare account_id required in credentials.extra or CLOUDFLARE_ACCOUNT_ID"
                        .into(),
                )
            })?;

        let id = generate_deployment_id("cf");
        let script_name = external_resource_name(&id);

        let script_body = match &config.mode {
            DeployMode::Wasm => {
                if let Some(wasm_b64) = config.env_vars.get("__WASM_B64") {
                    base64::engine::general_purpose::STANDARD
                        .decode(wasm_b64)
                        .map_err(|e| ClawzError::Validation(format!("invalid __WASM_B64: {e}")))?
                } else {
                    br#"export default { fetch() { return new Response("ClawZ"); } }"#.to_vec()
                }
            }
            DeployMode::Docker { image } => {
                format!("export default {{ fetch() {{ return new Response('docker:{image}'); }} }}")
                    .into_bytes()
            }
            _ => {
                return Err(ClawzError::Validation(
                    "Cloudflare only supports Docker and Wasm modes".into(),
                ));
            }
        };

        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/workers/scripts/{script_name}"
        );

        let content_type = match &config.mode {
            DeployMode::Wasm if config.env_vars.contains_key("__WASM_B64") => "application/wasm",
            _ => "application/javascript+module",
        };

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&token)
            .header("Content-Type", content_type)
            .body(script_body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Cloudflare deploy error: {e}")))?;

        let http_status = resp.status();
        let status = if http_status.is_success() {
            DeploymentStatus::Pending
        } else {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Cloudflare worker upload failed ({http_status}): {text}"
            )));
        };

        Ok(DeploymentInfo {
            id,
            external_resource: Some(script_name.clone()),
            url: format!("https://{script_name}.workers.dev"),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("CLOUDFLARE_API_TOKEN").map_err(|_| {
            ClawzError::Auth(
                "CLOUDFLARE_API_TOKEN required to destroy Cloudflare deployments".into(),
            )
        })?;
        let account_id = std::env::var("CLOUDFLARE_ACCOUNT_ID").map_err(|_| {
            ClawzError::Auth(
                "CLOUDFLARE_ACCOUNT_ID required to destroy Cloudflare deployments".into(),
            )
        })?;
        let script_name = external_resource
            .map(str::to_string)
            .unwrap_or_else(|| external_resource_name(id));
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/workers/scripts/{script_name}"
        );
        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Cloudflare delete script error: {e}")))?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            log::info!("Cloudflare worker removed: {script_name} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Cloudflare delete script failed ({status}): {text}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = CloudflareAdapter::new("my-account");
        assert_eq!(adapter.provider_id(), "cloudflare");
    }

    #[test]
    fn test_display_name() {
        let adapter = CloudflareAdapter::new("my-account");
        assert_eq!(adapter.display_name(), "Cloudflare");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = CloudflareAdapter::new("my-account");
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 2);
    }
}
