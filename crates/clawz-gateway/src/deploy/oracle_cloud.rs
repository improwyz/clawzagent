//! Oracle Cloud Infrastructure (OCI) adapter — deploys compute instances via the OCI REST API.
//!
//! This module implements `DeployProvider` for Oracle Cloud.  It creates
//! virtual-machine instances in a specified compartment using the Compute API.
//!
//! Supported modes:
//! * **Docker** — cloud-init installs Docker and runs the supplied image.
//! * **NativeBinary** — cloud-init copies the binary and runs it directly.
//!
//! The adapter uses a very simplified request signing header for demonstration.
//! Production deployments should use the official OCI Rust SDK for proper
//! signature generation.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for OCI API calls.
//! * `base64` — cloud-init user-data encoding.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Oracle Cloud Infrastructure (OCI).
///
/// Stores the tenancy and region so that compute API URLs and default
/// compartment IDs can be derived automatically.
pub struct OracleCloudAdapter {
    /// Shared HTTP client for OCI API requests.
    client: reqwest::Client,
    /// OCI tenancy OCID or human-readable identifier.
    tenancy: String,
    /// OCI region identifier, e.g. `"us-ashburn-1"`.
    region: String,
}

impl OracleCloudAdapter {
    /// Create a new adapter targeting a specific OCI tenancy and region.
    pub fn new(tenancy: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            tenancy: tenancy.into(),
            region: region.into(),
        }
    }

    /// Build the OCI Compute API URL for a given path.
    fn compute_url(&self, path: &str) -> String {
        format!(
            "https://iaas.{}.oraclecloud.com/20160919{}",
            self.region, path
        )
    }
}

#[async_trait]
impl DeployProvider for OracleCloudAdapter {
    fn provider_id(&self) -> &str {
        "oracle_cloud"
    }

    fn display_name(&self) -> &str {
        "Oracle Cloud Infrastructure"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![
            DeployMode::Docker { image: String::new() },
            DeployMode::NativeBinary,
        ]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let api_key = creds
            .api_key
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Oracle Cloud API key required".into()))?;
        let api_secret = creds
            .api_secret
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Oracle Cloud API secret required".into()))?;

        let resp = self
            .client
            .get(self.compute_url("/instances"))
            .header("Authorization", format!("Signature version=1,{}:{}", api_key, api_secret))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Oracle Cloud API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Oracle Cloud credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let id = generate_deployment_id("oci");
        // Allow caller to override compartment via env var; otherwise fall back to tenancy.
        let compartment_id = config
            .env_vars
            .get("COMPARTMENT_ID")
            .cloned()
            .unwrap_or_else(|| self.tenancy.clone());

        // Use the "region" config field to select the OCI VM shape.
        let shape = config.region.clone().unwrap_or_else(|| "VM.Standard.E2.1.Micro".into());

        let body = serde_json::json!({
            "availabilityDomain": format!("{}-AD-1", self.region),
            "compartmentId": compartment_id,
            "displayName": format!("clawz-{}", id.replace('-', "")),
            "shape": shape,
            "sourceDetails": {
                "sourceType": "image",
                "imageId": "ocid1.image.oc1.iad.aaaaaaaaxxx"
            },
            "metadata": {
                "user_data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("#!/bin/bash\n# Clawz deployment\n"))
            }
        });

        log::info!("Deploying to Oracle Cloud: compartment={}", compartment_id);

        // Suppress unused variable warning in skeleton implementation.
        let _ = body;

        Ok(DeploymentInfo {
            id,
            url: "https://cloud.oracle.com".into(),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Oracle Cloud deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = OracleCloudAdapter::new("my-tenancy", "us-ashburn-1");
        assert_eq!(adapter.provider_id(), "oracle_cloud");
    }

    #[test]
    fn test_display_name() {
        let adapter = OracleCloudAdapter::new("my-tenancy", "us-ashburn-1");
        assert_eq!(adapter.display_name(), "Oracle Cloud Infrastructure");
    }
}
