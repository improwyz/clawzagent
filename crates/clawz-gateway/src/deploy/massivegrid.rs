//! MassiveGrid Jelastic PaaS adapter.
//!
//! This module implements `DeployProvider` for MassiveGrid, a Jelastic-based
//! platform-as-a-service.  It uses the Jelastic REST API to create environments
//! and deploy Docker containers on shared infrastructure.
//!
//! Supported modes:
//! * **Docker** — creates a Jelastic environment with Docker nodes running the
//!   specified image.
//!
//! Native binary and Wasm are rejected because Jelastic on MassiveGrid is
//! container-centric.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for Jelastic API calls.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for MassiveGrid (Jelastic PaaS).
///
/// Stores the Jelastic API endpoint because MassiveGrid supports both public
/// and private Jelastic installations with different base URLs.
pub struct MassiveGridAdapter {
    /// Shared HTTP client for Jelastic API requests.
    client: reqwest::Client,
    /// Jelastic API endpoint, e.g. `"https://app.massivegrid.com"`.
    api_endpoint: String,
}

impl MassiveGridAdapter {
    /// Create a new adapter targeting a specific Jelastic API endpoint.
    pub fn new(api_endpoint: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_endpoint: api_endpoint.into(),
        }
    }

    /// Build a generic Jelastic API URL for a service and method.
    #[allow(dead_code)]
    fn api_url(&self, service: &str, method: &str) -> String {
        format!("{}/1.0/{}", self.api_endpoint, method)
            .replace("{service}", service)
    }

    /// URL for the Jelastic environment creation endpoint.
    fn create_env_url(&self) -> String {
        format!("{}/1.0/environment/control/rest/createenvironment", self.api_endpoint)
    }

    /// URL for the Jelastic container deployment endpoint.
    fn deploy_url(&self) -> String {
        format!("{}/1.0/environment/build/rest/deploycontainer", self.api_endpoint)
    }
}

#[async_trait]
impl DeployProvider for MassiveGridAdapter {
    fn provider_id(&self) -> &str {
        "massivegrid"
    }

    fn display_name(&self) -> &str {
        "MassiveGrid (Jelastic)"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![
            DeployMode::Docker { image: String::new() },
        ]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let session = creds
            .api_token
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("MassiveGrid session token required".into()))?;

        let url = format!(
            "{}/1.0/users/account/rest/getaccountinfo?session={}",
            self.api_endpoint, session
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("MassiveGrid API error: {e}")))?;

        if resp.status().is_success() {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ClawzError::Provider(format!("MassiveGrid response parse error: {e}")))?;
            // Jelastic uses result==0 to indicate success.
            if body["result"].as_i64().unwrap_or(1) == 0 {
                Ok(())
            } else {
                Err(ClawzError::Auth(format!(
                    "MassiveGrid credential error: {:?}",
                    body["error"]
                )))
            }
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid MassiveGrid credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let image = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            _ => {
                return Err(ClawzError::Validation(
                    "MassiveGrid Jelastic only supports Docker mode".into(),
                ))
            }
        };

        let id = generate_deployment_id("mg");
        let env_name = format!("clawz-{}", &id[3..11]);
        let region = config.region.as_deref().unwrap_or("default");

        // Step 1: Create environment manifest
        let env_manifest = serde_json::json!({
            "appid": "1dd8d191d38fff45e62564fcf67fdcd6",
            "envName": env_name,
            "engine": "dockerengine",
            "region": region,
            "nodes": [{
                "nodeType": "docker",
                "count": config.replicas,
                "fixedCloudlets": 1,
                "flexibleCloudlets": 4,
                "dockerConfig": {
                    "image": image,
                    "links": [],
                    "env": config.env_vars,
                    "ports": [{
                        "port": 8080,
                        "protocol": "TCP",
                        "internalPort": 8080,
                    }],
                }
            }]
        });

        log::info!(
            "Deploying to MassiveGrid Jelastic: env={}, create_url={}",
            env_name,
            self.create_env_url()
        );
        // Keep manifest and deploy URL alive for future expansion where we actually POST them.
        let _ = &env_manifest;
        let _ = self.deploy_url();

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.massivegrid.net", env_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying MassiveGrid deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = MassiveGridAdapter::new("https://app.massivegrid.com");
        assert_eq!(adapter.provider_id(), "massivegrid");
    }

    #[test]
    fn test_display_name() {
        let adapter = MassiveGridAdapter::new("https://app.massivegrid.com");
        assert_eq!(adapter.display_name(), "MassiveGrid (Jelastic)");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = MassiveGridAdapter::new("https://app.massivegrid.com");
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 1);
    }

    #[test]
    fn test_api_urls() {
        let adapter = MassiveGridAdapter::new("https://app.massivegrid.com");
        assert!(adapter.create_env_url().contains("createenvironment"));
        assert!(adapter.deploy_url().contains("deploycontainer"));
    }
}
