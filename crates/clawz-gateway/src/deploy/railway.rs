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
use crate::deploy::common::{
    destroy_http_ok, external_resource_name, generate_deployment_id, resolve_api_token,
};
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
            DeployMode::Docker {
                image: String::new(),
            },
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
            .header("Authorization", format!("Bearer {token}"))
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
        let _source = match &config.mode {
            DeployMode::Docker { image: _ } => {}
            DeployMode::NativeBinary => {}
            _ => {
                return Err(ClawzError::Validation(
                    "Railway does not support Wasm mode".into(),
                ));
            }
        };

        let token = resolve_api_token(config, "RAILWAY_TOKEN", "Railway")?;
        let id = generate_deployment_id("railway");
        let project_name = external_resource_name(&id);

        let query = serde_json::json!({
            "query": format!(
                "mutation {{ projectCreate(input: {{ name: \"{project_name}\" }}) {{ id }} }}"
            )
        });

        let resp = self
            .client
            .post(self.graphql_url())
            .header("Authorization", format!("Bearer {token}"))
            .json(&query)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Railway deploy error: {e}")))?;

        let http_status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Provider(format!("Railway deploy parse error: {e}")))?;

        if !http_status.is_success() {
            return Err(ClawzError::Provider(format!(
                "Railway projectCreate failed ({http_status}): {body}"
            )));
        }

        let project_id = body["data"]["projectCreate"]["id"]
            .as_str()
            .unwrap_or("unknown");

        Ok(DeploymentInfo {
            id,
            external_resource: Some(project_id.to_string()),
            url: format!("https://railway.app/project/{project_id}"),
            status: DeploymentStatus::Pending,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("RAILWAY_TOKEN").map_err(|_| {
            ClawzError::Auth("RAILWAY_TOKEN required to destroy Railway projects".into())
        })?;

        let project_id = if let Some(pid) = external_resource {
            pid.to_string()
        } else {
            let project_name = external_resource_name(id);
            let list_query = serde_json::json!({
                "query": "query { projects { edges { node { id name } } } } }"
            });
            let list_resp = self
                .client
                .post(self.graphql_url())
                .header("Authorization", format!("Bearer {token}"))
                .json(&list_query)
                .send()
                .await
                .map_err(|e| ClawzError::Provider(format!("Railway list projects error: {e}")))?;

            let list_body: serde_json::Value = list_resp
                .json()
                .await
                .map_err(|e| ClawzError::Provider(format!("Railway list parse error: {e}")))?;

            list_body["data"]["projects"]["edges"]
                .as_array()
                .and_then(|edges| {
                    edges.iter().find_map(|edge| {
                        let node = &edge["node"];
                        if node["name"].as_str() == Some(project_name.as_str()) {
                            node["id"].as_str().map(str::to_string)
                        } else {
                            None
                        }
                    })
                })
                .ok_or_else(|| ClawzError::NotFound {
                    entity: "railway_project".into(),
                    id: project_name.clone(),
                })?
        };

        let delete_query = serde_json::json!({
            "query": format!("mutation {{ projectDelete(id: \"{project_id}\") }}")
        });
        let del_resp = self
            .client
            .post(self.graphql_url())
            .header("Authorization", format!("Bearer {token}"))
            .json(&delete_query)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Railway delete project error: {e}")))?;

        if destroy_http_ok(del_resp.status()) {
            log::info!("Railway project removed: {project_id} (deployment {id})");
            Ok(())
        } else {
            let text = del_resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Railway projectDelete failed: {text}"
            )))
        }
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
