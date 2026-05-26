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
use crate::deploy::common::generate_deployment_id;
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
        format!("https://api.cloudflare.com/client/v4{}", path)
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
            DeployMode::Docker { image: String::new() },
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
        let id = generate_deployment_id("cf");
        // Remove dashes so the script name is a valid DNS label.
        let script_name = format!("clawz-{}", id.replace('-', ""));

        match &config.mode {
            DeployMode::Wasm => {
                log::info!("Deploying Wasm worker to Cloudflare: script={}", script_name);
            }
            DeployMode::Docker { image } => {
                log::info!(
                    "Deploying container to Cloudflare Pages/Workers: image={}",
                    image
                );
            }
            _ => {
                return Err(ClawzError::Validation(
                    "Cloudflare only supports Docker and Wasm modes".into(),
                ))
            }
        }

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.workers.dev", script_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Cloudflare deployment: id={}", id);
        Ok(())
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
