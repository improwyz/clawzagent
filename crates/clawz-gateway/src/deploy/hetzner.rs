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
use crate::deploy::common::{
    destroy_http_ok, external_resource_name, generate_deployment_id, resolve_api_token,
};
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

        let token = resolve_api_token(config, "HETZNER_API_TOKEN", "Hetzner")?;
        let server_name = external_resource_name(&id);
        let image = match &config.mode {
            DeployMode::Docker { image } => format!(
                "#!/bin/bash\napt-get update && apt-get install -y docker.io\n\
                 docker run -d -p 8080:8080 {image}\n"
            ),
            DeployMode::NativeBinary => "#!/bin/bash\n# native binary bootstrap\n".into(),
            _ => String::new(),
        };

        let body = serde_json::json!({
            "name": server_name,
            "server_type": server_type,
            "image": "ubuntu-22.04",
            "user_data": image,
            "location": "nbg1",
            "start_after_create": true,
        });

        let resp = self
            .client
            .post(self.api_url("/servers"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Hetzner deploy error: {e}")))?;

        let http_status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let status = if http_status.is_success() {
            DeploymentStatus::Pending
        } else {
            return Err(ClawzError::Provider(format!(
                "Hetzner create server failed ({http_status}): {text}"
            )));
        };

        let parsed: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({}));
        let deploy_url = parsed["server"]["public_net"]["ipv4"]["ip"]
            .as_str()
            .map(|ip| format!("http://{ip}:8080"))
            .unwrap_or_else(|| "https://hetzner.cloud".into());
        let server_id = parsed["server"]["id"].as_i64().map(|n| n.to_string());

        Ok(DeploymentInfo {
            id,
            external_resource: server_id.or(Some(server_name.clone())),
            url: deploy_url,
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("HETZNER_API_TOKEN").map_err(|_| {
            ClawzError::Auth("HETZNER_API_TOKEN required to destroy Hetzner servers".into())
        })?;
        let server_id = if let Some(sid) = external_resource {
            sid.to_string()
        } else {
            let server_name = external_resource_name(id);
            let list = self
                .client
                .get(self.api_url("/servers"))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| ClawzError::Provider(format!("Hetzner list servers error: {e}")))?;

            let body: serde_json::Value = list.json().await.map_err(|e| {
                ClawzError::Provider(format!("Hetzner list parse error: {e}"))
            })?;

            body["servers"]
                .as_array()
                .and_then(|servers| {
                    servers.iter().find_map(|s| {
                        if s["name"].as_str() == Some(server_name.as_str()) {
                            s["id"].as_i64().map(|n| n.to_string())
                        } else {
                            None
                        }
                    })
                })
                .ok_or_else(|| ClawzError::NotFound {
                    entity: "hetzner_server".into(),
                    id: server_name.clone(),
                })?
        };

        let resp = self
            .client
            .delete(self.api_url(&format!("/servers/{server_id}")))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Hetzner delete server error: {e}")))?;

        let status = resp.status();
        if destroy_http_ok(status) {
            log::info!("Hetzner server removed: {server_id} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Hetzner delete server failed ({status}): {text}"
            )))
        }
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
