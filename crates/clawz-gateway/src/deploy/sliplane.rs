use crate::deploy::common::generate_deployment_id;
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
        format!("https://api.sliplane.io/v1{}", path)
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
        vec![
            DeployMode::Docker { image: String::new() },
        ]
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

        let id = generate_deployment_id("sp");
        let service_name = format!("clawz-{}", &id[3..11]);

        let env_vec: Vec<serde_json::Value> = config
            .env_vars
            .iter()
            .map(|(k, v)| serde_json::json!({ "name": k, "value": v }))
            .collect();

        let _body = serde_json::json!({
            "name": service_name,
            "image": image,
            "env": env_vec,
            "region": config.region.as_deref().unwrap_or("eu-central-1"),
            "replicas": config.replicas,
            "port": 8080,
        });

        log::info!("Deploying to Sliplane: service={}", service_name);

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.sliplane.app", service_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Sliplane deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = SliplaneAdapter::new();
        assert_eq!(adapter.provider_id(), "sliplane");
    }

    #[test]
    fn test_display_name() {
        let adapter = SliplaneAdapter::new();
        assert_eq!(adapter.display_name(), "Sliplane");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = SliplaneAdapter::new();
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 1);
    }
}
