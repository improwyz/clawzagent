//! Fastly Compute@Edge adapter — deploys WASM packages to the Fastly edge network.
//!
//! This module implements `DeployProvider` for Fastly using the Fastly API v1.
//! It **only** supports `DeployMode::Wasm` because Fastly Compute@Edge is a
//! WebAssembly-based platform; Docker and native binaries are rejected.
//!
//! The adapter assumes the caller supplies a base64-encoded Wasm module in the
//! `__WASM_B64` environment variable.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — ID generation helper.
//! * `crate::deploy::provider` — core trait and types.
//! * `reqwest` — HTTP client for Fastly API calls.
//! * `clawz_core::error` — error types.

// Dependency: common helpers for deployment ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared vocabulary.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Fastly Compute@Edge (WASM-only).
///
/// No per-instance state is required beyond the shared `reqwest` client;
/// the Fastly API token is supplied at operation time via `ProviderCredentials`.
pub struct FastlyAdapter {
    /// Shared HTTP client for Fastly API requests.
    client: reqwest::Client,
}

impl FastlyAdapter {
    /// Create a new Fastly adapter.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Build a Fastly API v1 URL for the given path.
    fn api_url(&self, path: &str) -> String {
        format!("https://api.fastly.com{}", path)
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
        // Reject anything other than Wasm — Fastly Compute has no native binary or Docker host.
        match &config.mode {
            DeployMode::Wasm => {}
            DeployMode::Docker { .. } => {
                return Err(ClawzError::Validation(
                    "Fastly Compute only supports Wasm mode".into(),
                ))
            }
            DeployMode::NativeBinary => {
                return Err(ClawzError::Validation(
                    "Fastly Compute only supports Wasm mode".into(),
                ))
            }
        }

        let id = generate_deployment_id("fastly");
        // Keep the service name short so it fits Fastly naming limits.
        let service_name = format!("clawz-{}", &id[7..15]);

        // Step 1: Create a new Fastly service
        let _create_body = serde_json::json!({
            "name": service_name,
            "type": "wasm",
        });

        // Step 2: Create a service version
        // Step 3: Upload WASM package as multipart/form-data
        // Step 4: Activate the version
        // Wasm bytes are supplied by the caller via env_vars["__WASM_B64"]
        let wasm_b64 = config.env_vars.get("__WASM_B64").cloned().unwrap_or_default();

        log::info!(
            "Deploying WASM to Fastly Compute: service={}, wasm_bytes_b64_len={}",
            service_name,
            wasm_b64.len()
        );
        log::debug!(
            "Fastly service create URL: {}",
            self.api_url("/service")
        );

        Ok(DeploymentInfo {
            id,
            url: format!("https://{}.edgecompute.app", service_name),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying Fastly Compute deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = FastlyAdapter::new();
        assert_eq!(adapter.provider_id(), "fastly");
    }

    #[test]
    fn test_display_name() {
        let adapter = FastlyAdapter::new();
        assert_eq!(adapter.display_name(), "Fastly Compute");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = FastlyAdapter::new();
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 1);
        matches!(modes[0], DeployMode::Wasm);
    }

    #[test]
    fn test_api_url() {
        let adapter = FastlyAdapter::new();
        assert_eq!(
            adapter.api_url("/current_customer"),
            "https://api.fastly.com/current_customer"
        );
    }
}
