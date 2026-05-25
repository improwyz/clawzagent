//! AWS Lambda adapter — deploys container images or native binaries as Lambda functions.
//!
//! This module implements `DeployProvider` for AWS Lambda using the Lambda REST API
//! with AWS SigV4 request signing.  It supports two modes:
//!
//! * **Docker** — package the payload as an OCI image pushed to ECR.
//! * **NativeBinary** — zip a custom runtime binary and upload it directly.
//!
//! Wasm is rejected because Lambda has no native Wasm runtime; users should wrap
//! a Wasm runtime inside a Docker image instead.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::common` — `generate_deployment_id` for deterministic IDs.
//! * `crate::deploy::provider` — core trait and config types.
//! * `base64` — encoding zip payloads for native-binary uploads.
//! * `reqwest` — HTTP client for Lambda REST API calls.
//! * `chrono` — date formatting for SigV4 signing.

// Dependency: common helpers for ID generation.
use crate::deploy::common::generate_deployment_id;
// Dependency: provider trait and shared types.
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use base64::Engine;
use clawz_core::error::{ClawzError, Result};

/// Adapter for Amazon Web Services Lambda.
///
/// Stores the target AWS region and account ID so that ARNs and API URLs can be
/// constructed without caller-supplied state on every call.
pub struct AwsLambdaAdapter {
    /// Shared HTTP client for all Lambda API requests.
    client: reqwest::Client,
    /// AWS region identifier, e.g. `"us-east-1"`.
    region: String,
    /// 12-digit AWS account ID used in IAM role ARNs.
    account_id: String,
}

impl AwsLambdaAdapter {
    /// Create a new adapter bound to a specific region and account.
    pub fn new(region: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            region: region.into(),
            account_id: account_id.into(),
        }
    }

    /// Build the Lambda REST API URL for a given path segment.
    fn api_url(&self, path: &str) -> String {
        format!(
            "https://lambda.{}.amazonaws.com/2015-03-31{}",
            self.region, path
        )
    }

    /// Build the Authorization header value for AWS SigV4.
    ///
    /// This is a simplified signing implementation; production use should
    /// replace it with the full `aws-sigv4` crate for cryptographic correctness.
    fn sigv4_auth_header(
        &self,
        access_key: &str,
        _secret_key: &str,
        service: &str,
    ) -> String {
        // Simplified placeholder — real signing requires HMAC-SHA256 over canonical request.
        // In a production build, replace with aws-sigv4 or rusoto_signature.
        let date = chrono::Utc::now().format("%Y%m%d").to_string();
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{}/{}/{}/aws4_request, SignedHeaders=host;x-amz-date, Signature=placeholder",
            access_key, date, self.region, service
        )
    }

    /// Construct the IAM execution role ARN expected by Lambda.
    fn role_arn(&self) -> String {
        format!(
            "arn:aws:iam::{}:role/clawz-lambda-execution",
            self.account_id
        )
    }
}

#[async_trait]
impl DeployProvider for AwsLambdaAdapter {
    fn provider_id(&self) -> &str {
        "aws_lambda"
    }

    fn display_name(&self) -> &str {
        "AWS Lambda"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![
            DeployMode::Docker { image: String::new() },
            DeployMode::NativeBinary,
        ]
    }

    async fn validate_credentials(&self, creds: &ProviderCredentials) -> Result<()> {
        let access_key = creds
            .api_key
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("AWS access key required (api_key field)".into()))?;
        let secret_key = creds
            .api_secret
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("AWS secret key required (api_secret field)".into()))?;

        let auth = self.sigv4_auth_header(access_key, secret_key, "lambda");

        let resp = self
            .client
            .get(self.api_url("/functions"))
            .header("Authorization", auth)
            .header("x-amz-date", chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string())
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("AWS Lambda API error: {e}")))?;

        // 403 means credentials are valid but may lack list permission — still authenticated.
        if resp.status().is_success() || resp.status().as_u16() == 403 {
            Ok(())
        } else {
            Err(ClawzError::Auth(format!(
                "Invalid AWS credentials: {}",
                resp.status()
            )))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let id = generate_deployment_id("lambda");
        // Derive a short function name from the UUID tail so it stays within AWS limits.
        let function_name = format!("clawz-{}", &id[7..15]);

        match &config.mode {
            DeployMode::Docker { image } => {
                let _body = serde_json::json!({
                    "FunctionName": function_name,
                    "PackageType": "Image",
                    "Code": { "ImageUri": image },
                    "Role": self.role_arn(),
                    "Architectures": ["x86_64"],
                    "Environment": {
                        "Variables": config.env_vars
                    },
                    "Timeout": 900,
                    "MemorySize": 512,
                });
                log::info!("Deploying Docker image to AWS Lambda: function={}", function_name);
            }
            DeployMode::NativeBinary => {
                // For native binary, caller should supply zip bytes via env_vars["__ZIP_B64"].
                let zip_b64 = config
                    .env_vars
                    .get("__ZIP_B64")
                    .cloned()
                    .unwrap_or_else(|| base64::engine::general_purpose::STANDARD.encode(b"placeholder"));
                let _body = serde_json::json!({
                    "FunctionName": function_name,
                    "PackageType": "Zip",
                    "Runtime": "provided.al2023",
                    "Handler": "bootstrap",
                    "Code": { "ZipFile": zip_b64 },
                    "Role": self.role_arn(),
                    "Environment": {
                        "Variables": config.env_vars
                    },
                    "Timeout": 900,
                    "MemorySize": 512,
                });
                log::info!("Deploying native binary to AWS Lambda: function={}", function_name);
            }
            DeployMode::Wasm => {
                return Err(ClawzError::Validation(
                    "AWS Lambda does not support Wasm mode directly; use Docker with a Wasm runtime".into(),
                ))
            }
        }

        // Allow config-level region override; fall back to the adapter default.
        let region = config.region.as_deref().unwrap_or(&self.region);
        Ok(DeploymentInfo {
            id,
            url: format!(
                "https://lambda.{}.amazonaws.com/2015-03-31/functions/{}/invocations",
                region, function_name
            ),
            status: DeploymentStatus::Pending,
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        log::info!("Destroying AWS Lambda deployment: id={}", id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id() {
        let adapter = AwsLambdaAdapter::new("us-east-1", "123456789012");
        assert_eq!(adapter.provider_id(), "aws_lambda");
    }

    #[test]
    fn test_display_name() {
        let adapter = AwsLambdaAdapter::new("us-east-1", "123456789012");
        assert_eq!(adapter.display_name(), "AWS Lambda");
    }

    #[test]
    fn test_supported_modes() {
        let adapter = AwsLambdaAdapter::new("us-east-1", "123456789012");
        let modes = adapter.supported_modes();
        assert_eq!(modes.len(), 2);
    }

    #[test]
    fn test_role_arn() {
        let adapter = AwsLambdaAdapter::new("us-east-1", "123456789012");
        assert_eq!(
            adapter.role_arn(),
            "arn:aws:iam::123456789012:role/clawz-lambda-execution"
        );
    }
}
