use crate::deploy::common::{
    destroy_http_ok, generate_deployment_id, resolve_api_token, short_service_name,
};
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

pub struct SliplaneAdapter {
    client: reqwest::Client,
}

impl SliplaneAdapter {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://api.sliplane.io/v1{path}")
    }
}

impl Default for SliplaneAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeployProvider for SliplaneAdapter {
    fn provider_id(&self) -> &str {
        "sliplane"
    }

    fn display_name(&self) -> &str {
        "Sliplane"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![DeployMode::Docker { image: String::new() }]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let token = creds
            .api_token
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Sliplane API token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("/services"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Sliplane API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Sliplane credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let image = match &config.mode {
            DeployMode::Docker { image } => image.clone(),
            _ => {
                return Err(ClawzError::Validation(
                    "Sliplane only supports Docker mode".into(),
                ))
            }
        };

        let token = resolve_api_token(config, "SLIPLANE_API_TOKEN", "Sliplane")?;
        let id = generate_deployment_id("sp");
        let service_name = short_service_name(&id);

        let env_vec: Vec<serde_json::Value> = config
            .env_vars
            .iter()
            .map(|(k, v)| serde_json::json!({ "name": k, "value": v }))
            .collect();

        let body = serde_json::json!({
            "name": service_name,
            "image": image,
            "env": env_vec,
            "region": config.region.as_deref().unwrap_or("eu-central-1"),
            "replicas": config.replicas,
            "port": 8080,
        });

        let resp = self
            .client
            .post(self.api_url("/services"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Sliplane deploy error: {e}")))?;

        let http_status = resp.status();
        let status = if http_status.is_success() || http_status.as_u16() == 409 {
            DeploymentStatus::Pending
        } else {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Sliplane create service failed ({http_status}): {text}"
            )));
        };

        Ok(DeploymentInfo {
            id,
            external_resource: Some(service_name.clone()),
            url: format!("https://{service_name}.sliplane.app"),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("SLIPLANE_API_TOKEN").map_err(|_| {
            ClawzError::Auth("SLIPLANE_API_TOKEN required to destroy Sliplane services".into())
        })?;
        let service_name = external_resource
            .map(str::to_string)
            .unwrap_or_else(|| short_service_name(id));
        let url = self.api_url(&format!("/services/{service_name}"));

        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Sliplane delete error: {e}")))?;

        let status = resp.status();
        if destroy_http_ok(status) {
            log::info!("Sliplane service removed: {service_name} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Sliplane delete failed ({status}): {text}"
            )))
        }
    }
}
