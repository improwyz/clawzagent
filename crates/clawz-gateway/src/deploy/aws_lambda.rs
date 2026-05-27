//! AWS Lambda adapter — deploys container images or native binaries as Lambda functions.

use crate::deploy::common::{destroy_http_ok, generate_deployment_id, lambda_function_name};
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use crate::deploy::sigv4::sign_headers;
use async_trait::async_trait;
use base64::Engine;
use clawz_core::error::{ClawzError, Result};
use std::collections::HashMap;

/// Adapter for Amazon Web Services Lambda.
pub struct AwsLambdaAdapter {
    client: reqwest::Client,
    region: String,
    account_id: String,
}

impl AwsLambdaAdapter {
    pub fn new(region: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            region: region.into(),
            account_id: account_id.into(),
        }
    }

    fn api_host(&self, region: &str) -> String {
        format!("lambda.{region}.amazonaws.com")
    }

    fn api_url(&self, region: &str, path: &str) -> String {
        format!("https://{}/2015-03-31{path}", self.api_host(region))
    }

    fn role_arn(&self) -> String {
        format!(
            "arn:aws:iam::{}:role/clawz-lambda-execution",
            self.account_id
        )
    }

    fn resolve_credentials(config: &DeployConfig) -> Result<(String, String)> {
        if let Some(ref creds) = config.credentials {
            let access_key = creds.api_key.clone().ok_or_else(|| {
                ClawzError::Auth("AWS access key required (api_key field)".into())
            })?;
            let secret_key = creds.api_secret.clone().ok_or_else(|| {
                ClawzError::Auth("AWS secret key required (api_secret field)".into())
            })?;
            return Ok((access_key, secret_key));
        }
        let access_key = std::env::var("AWS_ACCESS_KEY_ID").map_err(|_| {
            ClawzError::Auth(
                "AWS credentials required: set config.credentials or AWS_ACCESS_KEY_ID".into(),
            )
        })?;
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").map_err(|_| {
            ClawzError::Auth(
                "AWS credentials required: set config.credentials or AWS_SECRET_ACCESS_KEY".into(),
            )
        })?;
        Ok((access_key, secret_key))
    }

    async fn signed_request(
        &self,
        region: &str,
        method: &str,
        path: &str,
        payload: &[u8],
        access_key: &str,
        secret_key: &str,
    ) -> Result<reqwest::Response> {
        let host = self.api_host(region);
        let url = format!("https://{host}/2015-03-31{path}");
        let signed = sign_headers(
            access_key, secret_key, region, "lambda", method, &host, path, "", payload,
        );

        let mut req = match method {
            "POST" => self.client.post(&url),
            "GET" => self.client.get(&url),
            "DELETE" => self.client.delete(&url),
            other => {
                return Err(ClawzError::Internal(format!(
                    "unsupported HTTP method for Lambda: {other}"
                )));
            }
        };
        req = req.header("Content-Type", "application/json");
        for (k, v) in signed {
            req = req.header(k, v);
        }
        if method != "GET" && !payload.is_empty() {
            req = req.body(payload.to_vec());
        }

        req.send()
            .await
            .map_err(|e| ClawzError::Provider(format!("AWS Lambda API error: {e}")))
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
            DeployMode::Docker {
                image: String::new(),
            },
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

        let region = creds
            .extra
            .get("region")
            .map(|s| s.as_str())
            .unwrap_or(&self.region);

        let resp = self
            .signed_request(region, "GET", "/functions", b"", access_key, secret_key)
            .await?;

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
        let (access_key, secret_key) = Self::resolve_credentials(config)?;
        let region = config.region.as_deref().unwrap_or(&self.region);
        let id = generate_deployment_id("lambda");
        let function_name = lambda_function_name(&id);

        let body = match &config.mode {
            DeployMode::Docker { image } => {
                let mut env_vars: HashMap<String, String> = config.env_vars.clone();
                env_vars.remove("__ZIP_B64");
                serde_json::json!({
                    "FunctionName": function_name,
                    "PackageType": "Image",
                    "Code": { "ImageUri": image },
                    "Role": self.role_arn(),
                    "Architectures": ["x86_64"],
                    "Environment": { "Variables": env_vars },
                    "Timeout": 900,
                    "MemorySize": 512,
                })
            }
            DeployMode::NativeBinary => {
                let zip_b64 = config
                    .env_vars
                    .get("__ZIP_B64")
                    .cloned()
                    .unwrap_or_else(|| {
                        base64::engine::general_purpose::STANDARD.encode(b"placeholder")
                    });
                let mut env_vars: HashMap<String, String> = config.env_vars.clone();
                env_vars.remove("__ZIP_B64");
                serde_json::json!({
                    "FunctionName": function_name,
                    "PackageType": "Zip",
                    "Runtime": "provided.al2023",
                    "Handler": "bootstrap",
                    "Code": { "ZipFile": zip_b64 },
                    "Role": self.role_arn(),
                    "Environment": { "Variables": env_vars },
                    "Timeout": 900,
                    "MemorySize": 512,
                })
            }
            DeployMode::Wasm => return Err(ClawzError::Validation(
                "AWS Lambda does not support Wasm mode directly; use Docker with a Wasm runtime"
                    .into(),
            )),
        };

        let payload = serde_json::to_vec(&body)
            .map_err(|e| ClawzError::Internal(format!("Lambda deploy JSON error: {e}")))?;

        let resp = self
            .signed_request(
                region,
                "POST",
                "/functions",
                &payload,
                &access_key,
                &secret_key,
            )
            .await?;

        let http_status = resp.status();
        let status = if http_status.is_success() || http_status.as_u16() == 409 {
            DeploymentStatus::Pending
        } else {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Lambda CreateFunction failed ({http_status}): {text}"
            )));
        };

        Ok(DeploymentInfo {
            id,
            external_resource: Some(function_name.clone()),
            url: self.api_url(region, &format!("/functions/{function_name}/invocations")),
            status,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, external_resource: Option<&str>) -> Result<()> {
        let (access_key, secret_key) = Self::resolve_credentials(&DeployConfig {
            mode: DeployMode::NativeBinary,
            env_vars: Default::default(),
            region: None,
            replicas: 1,
            credentials: None,
        })?;
        let region = std::env::var("AWS_REGION").unwrap_or_else(|_| self.region.clone());
        let function_name = external_resource
            .map(str::to_string)
            .unwrap_or_else(|| lambda_function_name(id));
        let path = format!("/functions/{function_name}");

        let resp = self
            .signed_request(
                region.as_str(),
                "DELETE",
                &path,
                b"",
                &access_key,
                &secret_key,
            )
            .await?;

        let status = resp.status();
        if destroy_http_ok(status) {
            log::info!("Lambda function removed: {function_name} (deployment {id})");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClawzError::Provider(format!(
                "Lambda DeleteFunction failed ({status}): {text}"
            )))
        }
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
    fn test_role_arn() {
        let adapter = AwsLambdaAdapter::new("us-east-1", "123456789012");
        assert_eq!(
            adapter.role_arn(),
            "arn:aws:iam::123456789012:role/clawz-lambda-execution"
        );
    }
}
