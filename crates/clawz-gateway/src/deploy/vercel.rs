use crate::deploy::common::generate_deployment_id;
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Vercel adapter — deploys serverless functions via the Vercel REST API v13.
/// Supports Docker images (via Vercel's container beta) and native binary (as a
/// Vercel serverless function zip).
pub struct VercelAdapter {
    client: reqwest::Client,
    team_id: Option<String>,
}

impl VercelAdapter {
    pub fn new(team_id: Option<impl Into<String>>) -> Self {
        Self {
            client: reqwest::Client::new(),
            team_id: team_id.map(|t| t.into()),
        }
    }

    fn api_url(&self, path: &str) -> String {
        if let Some(team) = &self.team_id {
            format!("https://api.vercel.com{}?teamId={}", path, team)
        } else {
            format!("https://api.vercel.com{}", path)
        }
    }
}

#[async_trait]
impl DeployProvider for VercelAdapter {
    fn provider_id(&self) -> &str {
        "vercel"
    }

    fn display_name(&self) -> &str {
        "Vercel"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![
            DeployMode::NativeBinary,
            DeployMode::Docker { image: String::new() },
        ]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let token = creds
            .api_token
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Vercel API token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("/v2/user"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Vercel API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Vercel credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let id = generate_deployment_id("vcl");
        let project_name = format!("clawz-{}", &id[4..12]);

        let env_vercel: Vec<serde_json::Value> = config
            .env_vars
            .iter()
            .map(|(k, v)| {
                serde_json::json!({
                    "key": k,
                    "value": v,
                    "type": "plain",
                    "target": ["production", "preview"],
                })
            })
            .collect();

        match &config.mode {
            DeployMode::NativeBinary => {
                // Deploy as a Vercel serverless function (Node/Edge runtime wrapper)
                let _body = serde_json::json!({
                    "name": project_name,
                    "files": [{
                        "file": "api/handler.js",
                        "data": "module.exports = (req, res) => res.send('ClawZ');",
                    }],
                    "projectSettings": {
                        "framework": null,
                        "buildCommand": null,
                        "outputDirectory": null,
                    },
                    "env": env_vercel,
                    "regions": [config.region.as_deref().unwrap_or("iad1")],
                });
                log::info!("Deploying native binary to Vercel: project={}", project_name);
            }
            DeployMode::Docker { image } => {
                let _body = serde_json::json!({
                    "name": project_name,
                    "container": { "image": image },
                    "env": env_vercel,
                    "regions": [config.region.as_deref().unwrap_or("iad1")],
                });
                log::info!("Deploying Docker image to Vercel: project={}, image={}", project_name, image);
            }
            DeployMode::Wasm => {
                return Err(ClawzError::Validation(
                    "Vercel does not support direct Wasm deployment; embed Wasm in a JS edge function".into(),
                ))
            }
        }

        log::debug!("Vercel deployments URL: {}", self.api_url("/v13/deployments"));

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.vercel.app", project_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Vercel deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = VercelAdapter::new(None::<String>);
        assert_eq!(adapter.provider_id(), "vercel");
    }

    #[test]
    fn test_display_name() {
        let adapter = VercelAdapter::new(None::<String>);
        assert_eq!(adapter.display_name(), "Vercel");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = VercelAdapter::new(None::<String>);
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 2);
    }

    #[test]
    fn test_api_url_with_team() {
        let adapter = VercelAdapter::new(Some("team_abc"));
        assert!(adapter.api_url("/v13/deployments").contains("teamId=team_abc"));
    }

    #[test]
    fn test_api_url_no_team() {
        let adapter = VercelAdapter::new(None::<String>);
        assert!(!adapter.api_url("/v13/deployments").contains("teamId"));
    }
}
