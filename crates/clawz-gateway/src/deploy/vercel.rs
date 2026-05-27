use crate::deploy::common::{
    destroy_http_ok, generate_deployment_id, resolve_api_token, vercel_project_name,
};
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Vercel adapter — deploys serverless functions via the Vercel REST API v13.
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
            format!("https://api.vercel.com{path}?teamId={team}")
        } else {
            format!("https://api.vercel.com{path}")
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
        let token = resolve_api_token(config, "VERCEL_TOKEN", "Vercel")?;
        let id = generate_deployment_id("vcl");
        let project_name = vercel_project_name(&id);
        let region = config.region.as_deref().unwrap_or("iad1");

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

        let body = match &config.mode {
            DeployMode::NativeBinary => serde_json::json!({
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
                "regions": [region],
            }),
            DeployMode::Docker { image } => serde_json::json!({
                "name": project_name,
                "container": { "image": image },
                "env": env_vercel,
                "regions": [region],
            }),
            DeployMode::Wasm => {
                return Err(ClawzError::Validation(
                    "Vercel does not support direct Wasm deployment; embed Wasm in a JS edge function".into(),
                ))
            }
        };

        let resp = self
            .client
            .post(self.api_url("/v13/deployments"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Vercel deploy error: {e}")))?;

        let http_status = resp.status();
        let status = if http_status.is_success() {
            DeploymentStatus::Pending
        } else {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Vercel deployment failed ({http_status}): {text}"
            )));
        };

        Ok(DeploymentInfo {
            id,
            external_resource: Some(project_name.clone()),
            url: format!("https://{project_name}.vercel.app"),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("VERCEL_TOKEN").map_err(|_| {
            ClawzError::Auth("VERCEL_TOKEN required to destroy Vercel projects".into())
        })?;
        let project_name = external_resource
            .map(str::to_string)
            .unwrap_or_else(|| vercel_project_name(id));
        let url = self.api_url(&format!("/v9/projects/{project_name}"));

        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Vercel delete project error: {e}")))?;

        let status = resp.status();
        if destroy_http_ok(status) {
            log::info!("Vercel project removed: {project_name} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Vercel delete project failed ({status}): {text}"
            )))
        }
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
    fn test_api_url_with_team() {
        let adapter = VercelAdapter::new(Some("team_abc"));
        assert!(adapter.api_url("/v13/deployments").contains("teamId=team_abc"));
    }
}
