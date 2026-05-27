//! Azure Functions adapter — deploys functions via the Azure Resource Management REST API.
//!
//! This module implements `DeployProvider` for Microsoft Azure Functions.  It uses
//! the Azure AD client-credentials flow to obtain a bearer token, then creates or
//! updates a Function App via the ARM REST API.
//!
//! Supported modes:
//! * **Docker** — deploys a custom Linux container image (`linuxFxVersion = DOCKER|…`).
//! * **NativeBinary** — deploys a .NET-isolated 8.0 function (placeholder; real usage
//!   would package the binary as a zip).
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for token and ARM API calls.
//! * `serde_json` — body construction for ARM requests.

// Dependency: common helpers for generating deployment IDs.
use crate::deploy::common::{azure_app_name, destroy_http_ok, generate_deployment_id};
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Microsoft Azure Functions.
///
/// Holds the subscription ID and resource group so that ARM URLs can be built
/// without re-supplying them on every operation.
pub struct AzureFunctionsAdapter {
    /// Shared HTTP client for token and ARM requests.
    client: reqwest::Client,
    /// Azure subscription GUID.
    subscription_id: String,
    /// Resource group that owns the Function App.
    resource_group: String,
}

impl AzureFunctionsAdapter {
    /// Create a new adapter targeting a specific subscription and resource group.
    pub fn new(subscription_id: impl Into<String>, resource_group: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            subscription_id: subscription_id.into(),
            resource_group: resource_group.into(),
        }
    }

    /// Base URL for the Azure Resource Manager (ARM) API.
    #[allow(dead_code)]
    fn management_url(&self, path: &str) -> String {
        format!("https://management.azure.com{}", path)
    }

    /// Build the ARM resource path for a Function App within this subscription + RG.
    fn function_app_url(&self, app_name: &str) -> String {
        format!(
            "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Web/sites/{}",
            self.subscription_id, self.resource_group, app_name
        )
    }

    /// Obtain a bearer token from Azure AD using the client-credentials OAuth2 flow.
    ///
    /// Returns the raw access token string on success.
    async fn get_bearer_token(
        &self,
        tenant_id: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String> {
        let token_url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            tenant_id
        );

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("scope", "https://management.azure.com/.default"),
        ];

        let resp = self
            .client
            .post(&token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Azure token request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(ClawzError::Auth(format!(
                "Azure AD token error: {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Provider(format!("Azure token parse error: {e}")))?;

        body["access_token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| ClawzError::Auth("Missing access_token in Azure response".into()))
    }

    fn azure_credentials(config: &DeployConfig) -> Result<(String, String, String)> {
        let creds = config.credentials.as_ref().ok_or_else(|| {
            ClawzError::Auth(
                "Azure credentials required in config.credentials (tenant_id in extra, api_key=client_id, api_secret=secret)".into(),
            )
        })?;
        let tenant_id =
            creds.extra.get("tenant_id").cloned().ok_or_else(|| {
                ClawzError::Auth("Azure tenant_id required in extra fields".into())
            })?;
        let client_id = creds
            .api_key
            .clone()
            .ok_or_else(|| ClawzError::Auth("Azure client_id required (api_key field)".into()))?;
        let client_secret = creds.api_secret.clone().ok_or_else(|| {
            ClawzError::Auth("Azure client_secret required (api_secret field)".into())
        })?;
        Ok((tenant_id, client_id, client_secret))
    }
}

#[async_trait]
impl DeployProvider for AzureFunctionsAdapter {
    fn provider_id(&self) -> &str {
        "azure_functions"
    }

    fn display_name(&self) -> &str {
        "Azure Functions"
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
        let tenant_id = creds
            .extra
            .get("tenant_id")
            .ok_or_else(|| ClawzError::Auth("Azure tenant_id required in extra fields".into()))?
            .clone();
        let client_id = creds
            .api_key
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Azure client_id required (api_key field)".into()))?
            .clone();
        let client_secret = creds
            .api_secret
            .as_ref()
            .ok_or_else(|| {
                ClawzError::Auth("Azure client_secret required (api_secret field)".into())
            })?
            .clone();

        // Attempt to get a bearer token — success proves credentials are valid.
        self.get_bearer_token(&tenant_id, &client_id, &client_secret)
            .await
            .map(|_| ())
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let image = match &config.mode {
            DeployMode::Docker { image } => Some(image.clone()),
            DeployMode::NativeBinary => None,
            DeployMode::Wasm => {
                return Err(ClawzError::Validation(
                    "Azure Functions does not support Wasm mode directly".into(),
                ));
            }
        };

        let (tenant_id, client_id, client_secret) = Self::azure_credentials(config)?;
        let bearer = self
            .get_bearer_token(&tenant_id, &client_id, &client_secret)
            .await?;

        let id = generate_deployment_id("az");
        let app_name = azure_app_name(&id);
        let resource_path = self.function_app_url(&app_name);
        let region = config.region.as_deref().unwrap_or("eastus");

        let site_config = if let Some(img) = image {
            serde_json::json!({
                "linuxFxVersion": format!("DOCKER|{img}"),
            })
        } else {
            serde_json::json!({
                "linuxFxVersion": "DOTNET-ISOLATED|8.0",
            })
        };

        let body = serde_json::json!({
            "location": region,
            "kind": "functionapp,linux",
            "properties": {
                "siteConfig": site_config,
                "reserved": true,
            }
        });

        let put_url = format!("https://management.azure.com{resource_path}?api-version=2022-03-01");
        let resp = self
            .client
            .put(&put_url)
            .bearer_auth(&bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Azure deploy request failed: {e}")))?;

        let http_status = resp.status();
        let status = if http_status.is_success() || http_status.as_u16() == 409 {
            DeploymentStatus::Pending
        } else {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Azure Function App deploy failed ({http_status}): {text}"
            )));
        };

        if !config.env_vars.is_empty() {
            let settings_url = format!(
                "https://management.azure.com{resource_path}/config/appsettings?api-version=2022-03-01"
            );
            let settings_body = serde_json::json!({
                "properties": config.env_vars,
            });
            let _ = self
                .client
                .put(&settings_url)
                .bearer_auth(&bearer)
                .json(&settings_body)
                .send()
                .await;
        }

        Ok(DeploymentInfo {
            id,
            external_resource: Some(app_name.clone()),
            url: format!("https://{app_name}.azurewebsites.net"),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let tenant_id = std::env::var("AZURE_TENANT_ID").map_err(|_| {
            ClawzError::Auth("AZURE_TENANT_ID required to destroy Azure Function Apps".into())
        })?;
        let client_id = std::env::var("AZURE_CLIENT_ID").map_err(|_| {
            ClawzError::Auth("AZURE_CLIENT_ID required to destroy Azure Function Apps".into())
        })?;
        let client_secret = std::env::var("AZURE_CLIENT_SECRET").map_err(|_| {
            ClawzError::Auth("AZURE_CLIENT_SECRET required to destroy Azure Function Apps".into())
        })?;

        let bearer = self
            .get_bearer_token(&tenant_id, &client_id, &client_secret)
            .await?;
        let app_name = external_resource
            .map(str::to_string)
            .unwrap_or_else(|| azure_app_name(id));
        let delete_url = format!(
            "https://management.azure.com{}?api-version=2022-03-01",
            self.function_app_url(&app_name)
        );

        let resp = self
            .client
            .delete(&delete_url)
            .bearer_auth(&bearer)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Azure delete request failed: {e}")))?;

        let status = resp.status();
        if destroy_http_ok(status) {
            log::info!("Azure Function App removed: {app_name} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Azure delete Function App failed ({status}): {text}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = AzureFunctionsAdapter::new("sub-123", "my-rg");
        assert_eq!(adapter.provider_id(), "azure_functions");
    }

    #[test]
    fn test_display_name() {
        let adapter = AzureFunctionsAdapter::new("sub-123", "my-rg");
        assert_eq!(adapter.display_name(), "Azure Functions");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = AzureFunctionsAdapter::new("sub-123", "my-rg");
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 2);
    }

    #[test]
    fn test_management_url() {
        let adapter = AzureFunctionsAdapter::new("sub-123", "my-rg");
        assert!(
            adapter
                .management_url("/foo")
                .starts_with("https://management.azure.com")
        );
    }
}
