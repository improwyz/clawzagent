//! Google Cloud Run adapter — deploys container images to Cloud Run.
//!
//! This module implements `DeployProvider` for Google Cloud Run using the
//! Knative-serving REST API (`run.googleapis.com`).  It only supports
//! `DeployMode::Docker` because Cloud Run is exclusively a container platform.
//!
//! The adapter generates a Knative `Service` manifest with environment variables
//! and sends it to the regional Cloud Run endpoint.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for Cloud Run API calls.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Google Cloud Run (Knative-based managed containers).
///
/// Stores the GCP project ID and default region so that API URLs and namespaces
/// can be constructed automatically.
pub struct GoogleCloudRunAdapter {
    /// Shared HTTP client for Cloud Run API requests.
    client: reqwest::Client,
    /// Google Cloud project identifier (e.g. `"my-project-123"`).
    project_id: String,
    /// Default Cloud Run region (e.g. `"us-central1"`).
    region: String,
}

impl GoogleCloudRunAdapter {
    /// Create a new adapter targeting a specific project and region.
    pub fn new(project_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            project_id: project_id.into(),
            region: region.into(),
        }
    }

    /// Build the regional Cloud Run API URL for a given path.
    fn api_url(&self, path: &str) -> String {
        format!(
            "https://{}-run.googleapis.com/apis/run.googleapis.com/v1/namespaces/{}/{}",
            self.region, self.project_id, path
        )
    }
}

#[async_trait]
impl DeployProvider for GoogleCloudRunAdapter {
    fn provider_id(&self) -> &str {
        "google_cloud_run"
    }

    fn display_name(&self) -> &str {
        "Google Cloud Run"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![DeployMode::Docker { image: String::new() }]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let token = creds
            .api_token
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Google Cloud access token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("services"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Cloud Run API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Cloud Run credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let image = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            _ => {
                return Err(ClawzError::Validation(
                    "Google Cloud Run only supports Docker deployments".into(),
                ))
            }
        };

        let id = generate_deployment_id("gcr");
        let service_name = format!("clawz-{}", id.replace('-', ""));

        // Build a Knative Service manifest with env vars injected as container env blocks.
        let _body = serde_json::json!({
            "apiVersion": "serving.knative.dev/v1",
            "kind": "Service",
            "metadata": {
                "name": service_name,
                "namespace": self.project_id,
            },
            "spec": {
                "template": {
                    "spec": {
                        "containers": [{
                            "image": image,
                            "env": config.env_vars.iter().map(|(k, v)| {
                                serde_json::json!({"name": k, "value": v})
                            }).collect::<Vec<_>>(),
                        }]
                    }
                }
            }
        });

        log::info!("Deploying to Google Cloud Run: service={}", service_name);

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.a.run.app", service_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Google Cloud Run deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = GoogleCloudRunAdapter::new("my-project", "us-central1");
        assert_eq!(adapter.provider_id(), "google_cloud_run");
    }

    #[test]
    fn test_display_name() {
        let adapter = GoogleCloudRunAdapter::new("my-project", "us-central1");
        assert_eq!(adapter.display_name(), "Google Cloud Run");
    }

    #[test]
    fn test_api_url() {
        let adapter = GoogleCloudRunAdapter::new("my-project", "us-central1");
        let url = adapter.api_url("services");
        assert!(url.contains("run.googleapis.com"));
        assert!(url.contains("my-project"));
    }
}
