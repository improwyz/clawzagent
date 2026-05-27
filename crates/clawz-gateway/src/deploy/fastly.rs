//! Fastly Compute@Edge adapter — deploys WASM packages to the Fastly edge network.

use crate::deploy::common::{fastly_service_key, generate_deployment_id, resolve_api_token};
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use base64::Engine;
use clawz_core::error::{ClawzError, Result};

pub struct FastlyAdapter {
    client: reqwest::Client,
}

impl FastlyAdapter {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://api.fastly.com{path}")
    }

    async fn fastly_request(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response> {
        let url = self.api_url(path);
        let mut req = self.client.request(method, &url).header("Fastly-Key", token);
        if let Some(ct) = content_type {
            req = req.header("Content-Type", ct);
        }
        if let Some(bytes) = body {
            req = req.body(bytes);
        }
        req.send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Fastly API error: {e}")))
    }
}

impl Default for FastlyAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeployProvider for FastlyAdapter {
    fn provider_id(&self) -> &str {
        "fastly"
    }

    fn display_name(&self) -> &str {
        "Fastly Compute"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![DeployMode::Wasm]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let token = creds
            .api_token
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Fastly API token required".into()))?;

        let resp = self
            .client
            .get(self.api_url("/current_customer"))
            .header("Fastly-Key", token.as_str())
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Fastly API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Fastly credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        match &config.mode {
            DeployMode::Wasm => {}
            _ => {
                return Err(ClawzError::Validation(
                    "Fastly Compute only supports Wasm mode".into(),
                ))
            }
        }

        let token = resolve_api_token(config, "FASTLY_API_TOKEN", "Fastly")?;
        let id = generate_deployment_id("fastly");
        let service_name = fastly_service_key(&id).ok_or_else(|| {
            ClawzError::Internal("invalid Fastly deployment id suffix".into())
        })?;

        let create_resp = self
            .fastly_request(
                &token,
                reqwest::Method::POST,
                "/service",
                Some(
                    serde_json::json!({
                        "name": service_name,
                        "type": "wasm",
                    })
                    .to_string()
                    .into_bytes(),
                ),
                Some("application/json"),
            )
            .await?;

        let create_status = create_resp.status();
        let create_body: serde_json::Value = if create_status.is_success() || create_status.as_u16() == 409 {
            create_resp
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({}))
        } else {
            let text = create_resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Fastly create service failed ({create_status}): {text}"
            )));
        };

        let service_id = create_body["id"]
            .as_str()
            .or_else(|| create_body["data"]["id"].as_str())
            .unwrap_or(&service_name);

        let version_resp = self
            .fastly_request(
                &token,
                reqwest::Method::POST,
                &format!("/service/{service_id}/version"),
                Some(
                    serde_json::json!({ "comment": "clawz deploy" })
                        .to_string()
                        .into_bytes(),
                ),
                Some("application/json"),
            )
            .await?;

        let version_status = version_resp.status();
        if !version_status.is_success() {
            let text = version_resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Fastly create version failed ({version_status}): {text}"
            )));
        }

        let version_body: serde_json::Value = version_resp.json().await.unwrap_or_default();
        let version_number = version_body["number"]
            .as_u64()
            .or_else(|| version_body["data"]["number"].as_u64())
            .unwrap_or(1);

        let wasm_bytes = if let Some(wasm_b64) = config.env_vars.get("__WASM_B64") {
            base64::engine::general_purpose::STANDARD
                .decode(wasm_b64)
                .map_err(|e| ClawzError::Validation(format!("invalid __WASM_B64: {e}")))?
        } else {
            br#"export default { fetch() { return new Response("ClawZ"); } }"#.to_vec()
        };

        let pkg_resp = self
            .fastly_request(
                &token,
                reqwest::Method::PUT,
                &format!("/service/{service_id}/version/{version_number}/package"),
                Some(wasm_bytes),
                Some("application/octet-stream"),
            )
            .await?;

        let pkg_status = pkg_resp.status();
        if !pkg_status.is_success() {
            let text = pkg_resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Fastly package upload failed ({pkg_status}): {text}"
            )));
        }

        let activate_resp = self
            .fastly_request(
                &token,
                reqwest::Method::PUT,
                &format!("/service/{service_id}/version/{version_number}/activate"),
                None,
                None,
            )
            .await?;

        let activate_status = activate_resp.status();
        let status = if activate_status.is_success() {
            DeploymentStatus::Pending
        } else {
            let text = activate_resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Fastly activate version failed ({activate_status}): {text}"
            )));
        };

        Ok(DeploymentInfo {
            id,
            external_resource: Some(service_name.clone()),
            url: format!("https://{service_name}.edgecompute.app"),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let token = std::env::var("FASTLY_API_TOKEN").map_err(|_| {
            ClawzError::Auth("FASTLY_API_TOKEN required to destroy Fastly deployments".into())
        })?;

        let service_key = external_resource
            .map(str::to_string)
            .or_else(|| fastly_service_key(id))
            .unwrap_or_else(|| id.to_string());

        let resp = self
            .fastly_request(
                &token,
                reqwest::Method::DELETE,
                &format!("/service/{service_key}"),
                None,
                None,
            )
            .await?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            log::info!("Fastly service removed: {service_key} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Fastly delete service failed ({status}): {text}"
            )))
        }
    }
}
