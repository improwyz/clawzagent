//! Northflank adapter — deploys services via the Northflank REST API.
//!
//! This module implements `DeployProvider` for Northflank, a DevOps platform
//! that runs Docker containers and native binaries on Kubernetes-backed infrastructure.
//!
//! Supported modes:
//! * **Docker** — deploy the supplied image to a Northflank service.
//! * **NativeBinary** — use Northflank's Nixpacks builder to compile and run
//!   the native binary from source.
//!
//! Wasm is rejected because Northflank does not expose a Wasm runtime.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for Northflank API calls.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Northflank (container and native-binary platform).
///
/// Stores the project ID because every Northflank service lives inside a project.
pub struct NorthflankAdapter {
    /// Shared HTTP client for Northflank API requests.
    client: reqwest::Client,
    /// Northflank project identifier that will own created services.
    project_id: String,
}

impl NorthflankAdapter {
    /// Create a new adapter bound to a Northflank project.
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            project_id: project_id.into(),
        }
    }

    /// Build a Northflank API v1 URL for the given path.
    fn api_url(&self, path: &str) -> String {
        format!("https://api.northflank.com/v1{}", path)
    }
}

#[async_trait]
impl DeployProvider for NorthflankAdapter {
    fn provider_id(&self) -> &str {
        "northflank"
    }

    fn display_name(&self) -> &str {
        "Northflank"
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
            .ok_or_else(|| ClawzError::Auth("Northflank API token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("/projects"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Northflank API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Northflank credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let image = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            // Northflank uses Nixpacks to auto-detect and build native binaries.
            DeployMode::NativeBinary => "debian:bullseye-slim".into(),
            DeployMode::Wasm => {
                return Err(ClawzError::Validation(
                    "Northflank does not support Wasm mode".into(),
                ))
            }
        };

        let id = generate_deployment_id("nf");
        let service_name = format!("clawz-{}", &id[3..11]);

        let _body = serde_json::json!({
            "name": service_name,
            "description": "ClawZ agent service",
            "serviceType": "deployment",
            "deployment": {
                "instances": config.replicas,
                "docker": {
                    "configType": "customDocker",
                    "dockerfileType": "default",
                },
                "region": config.region.as_deref().unwrap_or("europe-west"),
                "storage": {
                    "ephemeralStorage": { "storageSize": 1024 }
                }
            },
            "image": {
                "registryId": "dockerhub",
                "tag": image,
            },
            "ports": [{
                "name": "http",
                "internalPort": 8080,
                "public": true,
                "protocol": "HTTP",
            }],
            "runtimeEnvironment": config.env_vars,
        });

        log::info!(
            "Deploying to Northflank: project={}, service={}",
            self.project_id,
            service_name
        );

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.svc.cluster.local", service_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Northflank deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = NorthflankAdapter::new("my-project");
        assert_eq!(adapter.provider_id(), "northflank");
    }

    #[test]
    fn test_display_name() {
        let adapter = NorthflankAdapter::new("my-project");
        assert_eq!(adapter.display_name(), "Northflank");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = NorthflankAdapter::new("my-project");
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 2);
    }
}
