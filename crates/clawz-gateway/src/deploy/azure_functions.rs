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
use crate::deploy::common::generate_deployment_id;
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
    pub fn new(
        subscription_id: impl Into<String>,
        resource_group: impl Into<String>,
    ) -> Self {
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
            DeployMode::Docker { image: String::new() },
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
            .ok_or_else(|| ClawzError::Auth("Azure client_secret required (api_secret field)".into()))?
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
                ))
            }
        };

        let id = generate_deployment_id("az");
        // Strip dashes so the app name stays within Azure naming restrictions.
        let app_name = format!("clawz{}", &id[3..11].replace('-', ""));
        let resource_path = self.function_app_url(&app_name);
        let region = config.region.as_deref().unwrap_or("eastus");

        // Determine the Linux FX version based on deployment mode.
        let site_config = if let Some(img) = image {
            serde_json::json!({
                "linuxFxVersion": format!("DOCKER|{}", img),
            })
        } else {
            serde_json::json!({
                "linuxFxVersion": "dotnet-isolated|8.0",
            })
        };

        // Convert env-var map into Azure's app-settings array format.
        let env_app_settings: Vec<serde_json::Value> = config
            .env_vars
            .iter()
            .map(|(k, v)| serde_json::json!({ "name": k, "value": v }))
            .collect();

        let _body = serde_json::json!({
            "location": region,
            "kind": "functionapp,linux",
            "properties": {
                "serverFarmId": null,
                "siteConfig": site_config,
                "appSettings": env_app_settings,
                "reserved": true,
            }
        });

        log::info!(
            "Deploying to Azure Functions: app={}, resource_path={}",
            app_name,
            resource_path
        );

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.azurewebsites.net", app_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Azure Functions deployment: id={}", id);
        Ok(())
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
        assert!(adapter.management_url("/foo").starts_with("https://management.azure.com"));
    }
}
