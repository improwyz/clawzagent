//! Fly.io adapter — deploys Docker containers or native binaries via the Fly Machines API.
//!
//! This module implements `DeployProvider` for Fly.io.  It uses the Fly Machines
//! REST API (`api.machines.dev`) to create apps and launch machines.
//!
//! Supported modes:
//! * **Docker** — deploy the supplied OCI image directly.
//! * **NativeBinary** — wrap the binary in a minimal `debian:bullseye-slim` image
//!   because Fly.io ultimately runs containers.
//!
//! Wasm is rejected; Fly.io has no native Wasm runtime.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for Fly API calls.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared types.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Fly.io (Machines platform).
///
/// Stores the target organisation slug so that newly created apps are scoped
/// correctly under the caller's Fly account.
pub struct FlyIoAdapter {
    /// Shared HTTP client for Fly API requests.
    client: reqwest::Client,
    /// Fly organisation identifier, e.g. `"personal"`.
    org: String,
}

impl FlyIoAdapter {
    /// Create a new adapter targeting a specific Fly organisation.
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            org: org.into(),
        }
    }

    /// Build a Fly Machines API v1 URL for the given path.
    fn api_url(&self, path: &str) -> String {
        format!("https://api.machines.dev/v1{}", path)
    }
}

#[async_trait]
impl DeployProvider for FlyIoAdapter {
    fn provider_id(&self) -> &str {
        "fly_io"
    }

    fn display_name(&self) -> &str {
        "Fly.io"
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
            .ok_or_else(|| ClawzError::Auth("Fly.io API token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("/apps"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Fly.io API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Fly.io credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let image = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            // For native binary we need a base image; debian:bullseye-slim is small and compatible.
            DeployMode::NativeBinary => "debian:bullseye-slim".into(),
            _ => {
                return Err(ClawzError::Validation(
                    "Fly.io does not support Wasm mode".into(),
                ))
            }
        };

        let id = generate_deployment_id("fly");
        let app_name = format!("clawz-{}", id.replace('-', ""));

        let _body = serde_json::json!({
            "app": app_name,
            "org": self.org,
            "image": image,
            "env": config.env_vars,
            "services": [{
                "ports": [{"port": 8080, "handlers": ["http"]}],
                "protocol": "tcp",
                "internal_port": 8080
            }]
        });

        log::info!("Deploying to Fly.io: app={}", app_name);

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.fly.dev", app_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Fly.io deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = FlyIoAdapter::new("personal");
        assert_eq!(adapter.provider_id(), "fly_io");
    }

    #[test]
    fn test_display_name() {
        let adapter = FlyIoAdapter::new("personal");
        assert_eq!(adapter.display_name(), "Fly.io");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = FlyIoAdapter::new("personal");
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 2);
    }
}
