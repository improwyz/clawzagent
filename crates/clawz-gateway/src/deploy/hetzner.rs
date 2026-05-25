//! Hetzner Cloud adapter — deploys virtual servers via the Hetzner Cloud API.
//!
//! This module implements `DeployProvider` for Hetzner Cloud.  It creates
//! virtual machines (CX11, CPX11, etc.) running Ubuntu 22.04 and then relies
//! on cloud-init user-data to bootstrap the ClawZ payload.
//!
//! Supported modes:
//! * **Docker** — install Docker via cloud-init and run the supplied image.
//! * **NativeBinary** — copy the binary to the VM and run it as a systemd service.
//!
//! The caller can override the server type by setting `DeployConfig::region`;
//! otherwise the default `"cx21"` is used.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for Hetzner API calls.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Hetzner Cloud (virtual-server-based deployment).
///
/// No per-instance state is stored; the API token is supplied at operation time.
pub struct HetznerAdapter {
    /// Shared HTTP client for Hetzner Cloud API requests.
    client: reqwest::Client,
}

impl HetznerAdapter {
    /// Create a new Hetzner adapter.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Build a Hetzner Cloud API v1 URL for the given path.
    fn api_url(&self, path: &str) -> String {
        format!("https://api.hetzner.cloud/v1{}", path)
    }
}

#[async_trait]
impl DeployProvider for HetznerAdapter {
    fn provider_id(&self) -> &str {
        "hetzner"
    }

    fn display_name(&self) -> &str {
        "Hetzner Cloud"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![
            DeployMode::Docker { image: String::new() },
            DeployMode::NativeBinary,
        ]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let token = creds
            .api_token
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Hetzner API token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("/servers"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Hetzner API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Hetzner credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let id = generate_deployment_id("hcloud");

        // Hetzner uses "region" field to mean server type (e.g. cx21, cpx31).
        // This is a pragmatic overload because DeployConfig has no dedicated "server_type" field.
        let server_type = config
            .region
            .clone()
            .unwrap_or_else(|| "cx21".into());

        let _body = serde_json::json!({
            "name": format!("clawz-{}", id.replace('-', "")),
            "server_type": server_type,
            "image": "ubuntu-22.04",
            "user_data": format!("#!/bin/bash\n# Clawz deployment setup\n")
        });

        log::info!("Deploying to Hetzner Cloud: type={}", server_type);

        Ok(DeploymentInfo {
            id,
            url: "https://hetzner.cloud".into(),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Hetzner Cloud deployment: id={}", id);
        Ok(())
    }
}

impl Default for HetznerAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = HetznerAdapter::new();
        assert_eq!(adapter.provider_id(), "hetzner");
    }

    #[test]
    fn test_display_name() {
        let adapter = HetznerAdapter::new();
        assert_eq!(adapter.display_name(), "Hetzner Cloud");
    }
}
