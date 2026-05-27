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
use crate::deploy::common::{
    destroy_http_ok, external_resource_name, generate_deployment_id, resolve_api_token,
};
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
        let service_name = external_resource_name(&id);

        // Build a Knative Service manifest with env vars injected as container env blocks.
        let token = resolve_api_token(config, "GOOGLE_CLOUD_ACCESS_TOKEN", "Google Cloud")?;
        let region = config.region.as_deref().unwrap_or(&self.region);

        let body = serde_json::json!({
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

        let url = format!(
            "https://{region}-run.googleapis.com/apis/serving.knative.dev/v1/namespaces/{}/services",
            self.project_id
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Cloud Run deploy error: {e}")))?;

        let http_status = resp.status();
        let status = if http_status.is_success() || http_status.as_u16() == 409 {
            DeploymentStatus::Pending
        } else {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Cloud Run create service failed ({http_status}): {text}"
            )));
        };

        Ok(DeploymentInfo {
            id,
            external_resource: Some(service_name.clone()),
            url: format!("https://{service_name}-{region}.a.run.app"),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("GOOGLE_CLOUD_ACCESS_TOKEN").map_err(|_| {
            ClawzError::Auth(
                "GOOGLE_CLOUD_ACCESS_TOKEN required to destroy Cloud Run services".into(),
            )
        })?;
        let region = std::env::var("GOOGLE_CLOUD_REGION").unwrap_or_else(|_| self.region.clone());
        let service_name = external_resource
            .map(str::to_string)
            .unwrap_or_else(|| external_resource_name(id));
        let url = format!(
            "https://{region}-run.googleapis.com/apis/serving.knative.dev/v1/namespaces/{}/services/{service_name}",
            self.project_id
        );

        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Cloud Run delete error: {e}")))?;

        let status = resp.status();
        if destroy_http_ok(status) {
            log::info!("Cloud Run service removed: {service_name} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Cloud Run delete failed ({status}): {text}"
            )))
        }
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
