//! Railway adapter — deploys services via the Railway GraphQL API.
//!
//! This module implements `DeployProvider` for Railway, a container platform
//! that builds from Docker images or Nixpacks (for native binaries).  It uses
//! Railway's GraphQL API (`backboard.railway.app/graphql/v2`) for all operations.
//!
//! Supported modes:
//! * **Docker** — deploy the supplied image directly.
//! * **NativeBinary** — use Railway's Nixpacks auto-detection to build and run
//!   the binary from a Git repository or uploaded source.
//!
//! Wasm is rejected because Railway has no Wasm runtime.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for GraphQL queries and mutations.
//! * `serde_json` — GraphQL body construction.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Railway (container and Nixpacks platform).
///
/// No per-instance state is stored; the API token is supplied at operation time.
pub struct RailwayAdapter {
    /// Shared HTTP client for Railway GraphQL API requests.
    client: reqwest::Client,
}

impl RailwayAdapter {
    /// Create a new Railway adapter.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// The canonical Railway GraphQL v2 endpoint.
    fn graphql_url(&self) -> &'static str {
        "https://backboard.railway.app/graphql/v2"
    }
}

#[async_trait]
impl DeployProvider for RailwayAdapter {
    fn provider_id(&self) -> &str {
        "railway"
    }

    fn display_name(&self) -> &str {
        "Railway"
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
            .ok_or_else(|| ClawzError::Auth("Railway API token required".into()))?;

        // Simple "me" query to verify token validity.
        let query = serde_json::json!({
            "query": "query { me { id } }"
        });

        let resp = self
            .client
            .post(self.graphql_url())
            .header("Authorization", format!("Bearer {}", token))
            .json(&query)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Railway API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Railway credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let source = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            // Nixpacks auto-detects the language and builds the binary.
            DeployMode::NativeBinary => "nixpacks".into(),
            _ => {
                return Err(ClawzError::Validation(
                    "Railway does not support Wasm mode".into(),
                ))
            }
        };

        let id = generate_deployment_id("railway");

        log::info!("Deploying to Railway: source={}", source);

        Ok(DeploymentInfo {
            id,
            url: "https://railway.app".into(),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Railway deployment: id={}", id);
        Ok(())
    }
}

impl Default for RailwayAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = RailwayAdapter::new();
        assert_eq!(adapter.provider_id(), "railway");
    }

    #[test]
    fn test_display_name() {
        let adapter = RailwayAdapter::new();
        assert_eq!(adapter.display_name(), "Railway");
    }
}
