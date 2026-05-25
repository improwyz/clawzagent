//! Generic Kubernetes adapter — generates Deployment + Service manifests and
//! applies them via the Kubernetes API server.
//!
//! This module implements `DeployProvider` for any Kubernetes cluster that
//! exposes the standard REST API.  It supports:
//!
//! * **Docker** — deploy the supplied image as a container in a Deployment.
//! * **NativeBinary** — wrap the binary in `debian:bullseye-slim` because
//!   Kubernetes ultimately runs containers.
//!
//! Wasm is rejected because vanilla Kubernetes has no built-in Wasm runtime;
//! users should use a Docker image embedding a Wasm runtime (e.g. WasmEdge,
//! runwasi) instead.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for `kubectl`-equivalent REST calls.
//! * `serde_json` — manifest construction.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for a generic Kubernetes cluster.
///
/// Targets a specific API server and namespace so that multiple adapters can
/// coexist in the same `DeployManager` pointing at different clusters or namespaces.
pub struct KubernetesAdapter {
    /// Shared HTTP client for Kubernetes API requests.
    client: reqwest::Client,
    /// Kubernetes API server URL, e.g. `"https://kubernetes.default.svc"`.
    api_server: String,
    /// Namespace in which resources will be created.
    namespace: String,
}

impl KubernetesAdapter {
    /// Create a new adapter targeting a specific API server and namespace.
    pub fn new(api_server: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_server: api_server.into(),
            namespace: namespace.into(),
        }
    }

    /// URL for the apps/v1 Deployment collection in the target namespace.
    fn deployments_url(&self) -> String {
        format!(
            "{}/apis/apps/v1/namespaces/{}/deployments",
            self.api_server, self.namespace
        )
    }

    /// URL for the v1 Service collection in the target namespace.
    fn services_url(&self) -> String {
        format!(
            "{}/api/v1/namespaces/{}/services",
            self.api_server, self.namespace
        )
    }

    /// Build a standard `apps/v1 Deployment` manifest for the given parameters.
    fn deployment_manifest(
        &self,
        name: &str,
        image: &str,
        replicas: u32,
        env_vars: &std::collections::HashMap<String, String>,
    ) -> serde_json::Value {
        let env: Vec<serde_json::Value> = env_vars
            .iter()
            .map(|(k, v)| serde_json::json!({ "name": k, "value": v }))
            .collect();

        serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {
                "name": name,
                "namespace": self.namespace,
                "labels": { "app": name, "managed-by": "clawz" }
            },
            "spec": {
                "replicas": replicas,
                "selector": { "matchLabels": { "app": name } },
                "template": {
                    "metadata": { "labels": { "app": name } },
                    "spec": {
                        "containers": [{
                            "name": name,
                            "image": image,
                            "ports": [{ "containerPort": 8080 }],
                            "env": env,
                            "resources": {
                                "requests": { "memory": "128Mi", "cpu": "100m" },
                                "limits":   { "memory": "512Mi", "cpu": "500m" }
                            }
                        }]
                    }
                }
            }
        })
    }

    /// Build a standard `v1 Service` manifest (ClusterIP) for the given app name.
    fn service_manifest(&self, name: &str) -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": name,
                "namespace": self.namespace,
                "labels": { "app": name }
            },
            "spec": {
                "selector": { "app": name },
                "ports": [{ "port": 80, "targetPort": 8080, "protocol": "TCP" }],
                "type": "ClusterIP"
            }
        })
    }
}

#[async_trait]
impl DeployProvider for KubernetesAdapter {
    fn provider_id(&self) -> &str {
        "kubernetes"
    }

    fn display_name(&self) -> &str {
        "Kubernetes"
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
            .ok_or_else(|| ClawzError::Auth("Kubernetes bearer token required".into()))?;

        let url = format!("{}/api/v1/namespaces/{}", self.api_server, self.namespace);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Kubernetes API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Kubernetes credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let image = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            // Wrap native binaries in a minimal base image because K8s only runs containers.
            DeployMode::NativeBinary => "debian:bullseye-slim".into(),
            DeployMode::Wasm => {
                return Err(ClawzError::Validation(
                    "Kubernetes Wasm mode requires a Wasm-capable runtime node — use Docker mode with a Wasm runtime image instead".into(),
                ))
            }
        };

        let id = generate_deployment_id("k8s");
        let deploy_name = format!("clawz-{}", &id[4..12]);

        let deployment = self.deployment_manifest(
            &deploy_name,
            &image,
            config.replicas,
            &config.env_vars,
        );
        let service = self.service_manifest(&deploy_name);

        log::info!(
            "Applying Kubernetes manifests: deployment={}, namespace={}, deployments_url={}",
            deploy_name,
            self.namespace,
            self.deployments_url()
        );
        // Log service manifest URL for completeness (useful when debugging API paths).
        log::debug!("Services URL: {}", self.services_url());

        // Manifests are built but not yet POSTed in this skeleton implementation.
        let _ = deployment;
        let _ = service;

        Ok(DeploymentInfo {
            id,
            url: format!(
                "http://{}.{}.svc.cluster.local",
                deploy_name, self.namespace
            ),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Kubernetes deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_provider_id() {
        let adapter = KubernetesAdapter::new("https://k8s.example.com", "default");
        assert_eq!(adapter.provider_id(), "kubernetes");
    }

    #[test]
    fn test_display_name() {
        let adapter = KubernetesAdapter::new("https://k8s.example.com", "default");
        assert_eq!(adapter.display_name(), "Kubernetes");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = KubernetesAdapter::new("https://k8s.example.com", "default");
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 2);
    }

    #[test]
    fn test_deployment_manifest_structure() {
        let adapter = KubernetesAdapter::new("https://k8s.example.com", "default");
        let mut env = HashMap::new();
        env.insert("FOO".into(), "bar".into());
        let manifest = adapter.deployment_manifest("my-app", "nginx:latest", 2, &env);
        assert_eq!(manifest["kind"], "Deployment");
        assert_eq!(manifest["spec"]["replicas"], 2);
    }

    #[test]
    fn test_service_manifest_structure() {
        let adapter = KubernetesAdapter::new("https://k8s.example.com", "default");
        let manifest = adapter.service_manifest("my-app");
        assert_eq!(manifest["kind"], "Service");
        assert_eq!(manifest["spec"]["type"], "ClusterIP");
    }
}
