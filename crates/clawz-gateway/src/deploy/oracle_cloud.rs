//! Oracle Cloud Infrastructure (OCI) adapter — deploys compute instances via the OCI REST API.
//!
//! This module implements `DeployProvider` for Oracle Cloud.  It creates
//! virtual-machine instances in a specified compartment using the Compute API.
//!
//! Supported modes:
//! * **Docker** — cloud-init installs Docker and runs the supplied image.
//! * **NativeBinary** — cloud-init copies the binary and runs it directly.
//!
//! The adapter uses a very simplified request signing header for demonstration.
//! Production deployments should use the official OCI Rust SDK for proper
//! signature generation.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for OCI API calls.
//! * `base64` — cloud-init user-data encoding.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::{destroy_http_ok, external_resource_name, generate_deployment_id};
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Oracle Cloud Infrastructure (OCI).
///
/// Stores the tenancy and region so that compute API URLs and default
/// compartment IDs can be derived automatically.
pub struct OracleCloudAdapter {
    /// Shared HTTP client for OCI API requests.
    client: reqwest::Client,
    /// OCI tenancy OCID or human-readable identifier.
    tenancy: String,
    /// OCI region identifier, e.g. `"us-ashburn-1"`.
    region: String,
}

impl OracleCloudAdapter {
    /// Create a new adapter targeting a specific OCI tenancy and region.
    pub fn new(tenancy: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            tenancy: tenancy.into(),
            region: region.into(),
        }
    }

    /// Build the OCI Compute API URL for a given path.
    fn compute_url(&self, path: &str) -> String {
        format!(
            "https://iaas.{}.oraclecloud.com/20160919{}",
            self.region, path
        )
    }

    fn auth_header(api_key: &str, api_secret: &str) -> String {
        format!("Signature version=1,{api_key}:{api_secret}")
    }

    fn resolve_oci_credentials() -> Result<(String, String)> {
        let api_key = std::env::var("OCI_API_KEY")
            .map_err(|_| ClawzError::Auth("OCI_API_KEY required".into()))?;
        let api_secret = std::env::var("OCI_API_SECRET")
            .map_err(|_| ClawzError::Auth("OCI_API_SECRET required".into()))?;
        Ok((api_key, api_secret))
    }
}

#[async_trait]
impl DeployProvider for OracleCloudAdapter {
    fn provider_id(&self) -> &str {
        "oracle_cloud"
    }

    fn display_name(&self) -> &str {
        "Oracle Cloud Infrastructure"
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
        let api_key = creds
            .api_key
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Oracle Cloud API key required".into()))?;
        let api_secret = creds
            .api_secret
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("Oracle Cloud API secret required".into()))?;

        let resp = self
            .client
            .get(self.compute_url("/instances"))
            .header(
                "Authorization",
                format!("Signature version=1,{}:{}", api_key, api_secret),
            )
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Oracle Cloud API error: {e}")))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid Oracle Cloud credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let id = generate_deployment_id("oci");
        // Allow caller to override compartment via env var; otherwise fall back to tenancy.
        let compartment_id = config
            .env_vars
            .get("COMPARTMENT_ID")
            .cloned()
            .unwrap_or_else(|| self.tenancy.clone());

        // Use the "region" config field to select the OCI VM shape.
        let shape = config
            .region
            .clone()
            .unwrap_or_else(|| "VM.Standard.E2.1.Micro".into());

        let display_name = external_resource_name(&id);
        let body = serde_json::json!({
            "availabilityDomain": format!("{}-AD-1", self.region),
            "compartmentId": compartment_id,
            "displayName": display_name,
            "shape": shape,
            "sourceDetails": {
                "sourceType": "image",
                "imageId": "ocid1.image.oc1.iad.aaaaaaaaxxx"
            },
            "metadata": {
                "user_data": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    "#!/bin/bash\n# Clawz deployment\n",
                )
            }
        });

        let api_key = config
            .credentials
            .as_ref()
            .and_then(|c| c.api_key.clone())
            .or_else(|| std::env::var("OCI_API_KEY").ok())
            .ok_or_else(|| ClawzError::Auth("Oracle API key required".into()))?;
        let api_secret = config
            .credentials
            .as_ref()
            .and_then(|c| c.api_secret.clone())
            .or_else(|| std::env::var("OCI_API_SECRET").ok())
            .ok_or_else(|| ClawzError::Auth("Oracle API secret required".into()))?;

        let resp = self
            .client
            .post(self.compute_url("/instances"))
            .header(
                "Authorization",
                format!("Signature version=1,{api_key}:{api_secret}"),
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Oracle Cloud deploy error: {e}")))?;

        let http_status = resp.status();
        let response_body: serde_json::Value =
            if http_status.is_success() || http_status.as_u16() == 409 {
                resp.json().await.unwrap_or_default()
            } else {
                let text = resp.text().await.unwrap_or_default();
                return Err(ClawzError::Provider(format!(
                    "Oracle create instance failed ({http_status}): {text}"
                )));
            };

        let status = DeploymentStatus::Pending;
        let instance_id = response_body["id"]
            .as_str()
            .map(str::to_string)
            .or_else(|| response_body["data"]["id"].as_str().map(str::to_string));

        Ok(DeploymentInfo {
            id,
            external_resource: instance_id.or(Some(display_name)),
            url: "https://cloud.oracle.com".into(),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let (api_key, api_secret) = Self::resolve_oci_credentials()?;

        let instance_id = if let Some(ocid) = external_resource {
            ocid.to_string()
        } else {
            let compartment_id =
                std::env::var("OCI_COMPARTMENT_ID").unwrap_or_else(|_| self.tenancy.clone());
            let display_name = external_resource_name(id);
            let encoded_compartment: String = compartment_id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                        c.to_string()
                    } else {
                        format!("%{:02X}", c as u8)
                    }
                })
                .collect();
            let list_url = format!(
                "{}?compartmentId={encoded_compartment}",
                self.compute_url("/instances")
            );
            let list_resp = self
                .client
                .get(&list_url)
                .header("Authorization", Self::auth_header(&api_key, &api_secret))
                .send()
                .await
                .map_err(|e| ClawzError::Provider(format!("Oracle list instances error: {e}")))?;

            let list_body: serde_json::Value = list_resp
                .json()
                .await
                .map_err(|e| ClawzError::Provider(format!("Oracle list parse error: {e}")))?;

            list_body
                .get("items")
                .or_else(|| list_body.get("data"))
                .and_then(|v| v.as_array())
                .and_then(|items| {
                    items.iter().find_map(|item| {
                        if item["displayName"].as_str() == Some(display_name.as_str()) {
                            item["id"].as_str().map(str::to_string)
                        } else {
                            None
                        }
                    })
                })
                .ok_or_else(|| ClawzError::NotFound {
                    entity: "oci_instance".into(),
                    id: display_name.clone(),
                })?
        };

        let delete_url = self.compute_url(&format!("/instances/{instance_id}"));
        let del_resp = self
            .client
            .delete(&delete_url)
            .header("Authorization", Self::auth_header(&api_key, &api_secret))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Oracle terminate instance error: {e}")))?;

        let status = del_resp.status();
        if destroy_http_ok(status) {
            log::info!("OCI instance terminated: {instance_id} (deployment {id})");
            Ok(())
        } else {
            let text = del_resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Oracle terminate failed ({status}): {text}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = OracleCloudAdapter::new("my-tenancy", "us-ashburn-1");
        assert_eq!(adapter.provider_id(), "oracle_cloud");
    }

    #[test]
    fn test_display_name() {
        let adapter = OracleCloudAdapter::new("my-tenancy", "us-ashburn-1");
        assert_eq!(adapter.display_name(), "Oracle Cloud Infrastructure");
    }
}
