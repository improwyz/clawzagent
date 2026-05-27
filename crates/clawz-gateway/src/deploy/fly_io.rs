//! Fly.io adapter — deploys Docker containers via the Fly Machines API.

use crate::deploy::common::{external_resource_name, generate_deployment_id, resolve_api_token};
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

pub struct FlyIoAdapter {
    client: reqwest::Client,
    org: String,
}

impl FlyIoAdapter {
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            org: org.into(),
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://api.machines.dev/v1{path}")
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
        let token = resolve_api_token(config, "FLY_API_TOKEN", "Fly.io")?;
        let image = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            DeployMode::NativeBinary => "debian:bullseye-slim".into(),
            _ => {
                return Err(ClawzError::Validation(
                    "Fly.io does not support Wasm mode".into(),
                ))
            }
        };

        let id = generate_deployment_id("fly");
        let app_name = external_resource_name(&id);

        let create_app = serde_json::json!({
            "app_name": app_name,
            "org_slug": self.org,
        });

        let app_resp = self
            .client
            .post(self.api_url("/apps"))
            .bearer_auth(&token)
            .json(&create_app)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Fly.io create app error: {e}")))?;

        let app_status = app_resp.status();
        if !app_status.is_success() && app_status.as_u16() != 409 {
            let body = app_resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Fly.io create app failed ({app_status}): {body}"
            )));
        }

        let env: Vec<serde_json::Value> = config
            .env_vars
            .iter()
            .map(|(k, v)| json_env(k, v))
            .collect();

        let machine_config = serde_json::json!({
            "config": {
                "image": image,
                "env": env,
                "services": [{
                    "protocol": "tcp",
                    "internal_port": 8080,
                    "ports": [{"port": 80, "handlers": ["http"]}, {"port": 443, "handlers": ["tls", "http"]}],
                }],
                "guest": {
                    "cpu_kind": "shared",
                    "cpus": 1,
                    "memory_mb": 512
                }
            }
        });

        let machine_resp = self
            .client
            .post(self.api_url(&format!("/apps/{app_name}/machines")))
            .bearer_auth(&token)
            .json(&machine_config)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Fly.io create machine error: {e}")))?;

        let machine_status = machine_resp.status();
        let status = if machine_status.is_success() || machine_status.as_u16() == 409 {
            DeploymentStatus::Pending
        } else {
            let body = machine_resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Fly.io create machine failed ({machine_status}): {body}"
            )));
        };

        Ok(DeploymentInfo {
            id,
            external_resource: Some(app_name.clone()),
            url: format!("https://{app_name}.fly.dev"),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("FLY_API_TOKEN").map_err(|_| {
            ClawzError::Auth("FLY_API_TOKEN required to destroy Fly.io deployments".into())
        })?;
        let app_name = external_resource
            .map(str::to_string)
            .unwrap_or_else(|| external_resource_name(id));
        let resp = self
            .client
            .delete(self.api_url(&format!("/apps/{app_name}")))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Fly.io delete app error: {e}")))?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            log::info!("Fly.io app removed: {app_name} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Fly.io delete app failed ({status}): {text}"
            )))
        }
    }
}

fn json_env(key: &str, value: &str) -> serde_json::Value {
    serde_json::json!({ "name": key, "value": value })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = FlyIoAdapter::new("personal");
        assert_eq!(adapter.provider_id(), "fly_io");
    }
}
