//! Pipedrive connector for Clawz Gateway.
//!
//! Talks to the Pipedrive REST API v1 using an API key (`api_token`). Supports
//! deals, persons, organisations, activities, pipelines and stages.
//!
//! # Authentication
//! API key only. Pipedrive does not use OAuth2 for this connector implementation.
//!
//! # Cross-module dependencies
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Pipedrive connector.
///
/// All requests are sent to `https://api.pipedrive.com/v1` with the API key
/// injected as a query parameter (`api_token`) — the canonical Pipedrive
/// authentication style for v1.
pub struct PipedriveConnector {
    /// Pipedrive API key.
    api_key: String,
    /// Shared HTTP client.
    client: Client,
}

impl PipedriveConnector {
    /// Create a new Pipedrive connector with an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    /// Pipedrive v1 API root.
    fn base_url(&self) -> String {
        "https://api.pipedrive.com/v1".into()
    }
}

#[async_trait]
impl SaaSConnector for PipedriveConnector {
    fn platform_id(&self) -> &str {
        "pipedrive"
    }

    fn display_name(&self) -> &str {
        "Pipedrive"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth(
            "Pipedrive uses API key authentication".into(),
        ))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth(
            "Pipedrive uses API key authentication".into(),
        ))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let path = match obj {
            "deals" => "/deals",
            "persons" => "/persons",
            "organizations" => "/organizations",
            "activities" => "/activities",
            "pipelines" => "/pipelines",
            "stages" => "/stages",
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Pipedrive object: {obj}"
                )));
            }
        };
        // Pipedrive allows up to 500 items per page.
        let limit = filters.limit.unwrap_or(100).min(500);
        let resp = self
            .client
            .get(format!("{}{}", self.base_url(), path))
            .query(&[("api_token", &self.api_key), ("limit", &limit.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Pipedrive list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "deals" => "/deals",
            "persons" => "/persons",
            "organizations" => "/organizations",
            "activities" => "/activities",
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Pipedrive object: {obj}"
                )));
            }
        };
        let resp = self
            .client
            .post(format!("{}{}", self.base_url(), path))
            .query(&[("api_token", &self.api_key)])
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Pipedrive create failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"].clone())
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "deals" => format!("/deals/{}", id),
            "persons" => format!("/persons/{}", id),
            "organizations" => format!("/organizations/{}", id),
            "activities" => format!("/activities/{}", id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Pipedrive object: {obj}"
                )));
            }
        };
        let resp = self
            .client
            .put(format!("{}{}", self.base_url(), path))
            .query(&[("api_token", &self.api_key)])
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Pipedrive update failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"].clone())
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "deals" => format!("/deals/{}", id),
            "persons" => format!("/persons/{}", id),
            "organizations" => format!("/organizations/{}", id),
            "activities" => format!("/activities/{}", id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Pipedrive object: {obj}"
                )));
            }
        };
        let resp = self
            .client
            .delete(format!("{}{}", self.base_url(), path))
            .query(&[("api_token", &self.api_key)])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Pipedrive delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Pipedrive delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let path = match action {
            "search" => {
                let term = params["term"].as_str().unwrap_or("");
                format!("/itemSearch?api_token={}&term={}", self.api_key, term)
            }
            "merge_persons" => {
                let id = params["id"].as_str().unwrap_or("");
                format!("/persons/{}/merge?api_token={}", id, self.api_key)
            }
            _ => format!("/{}?api_token={}", action, self.api_key),
        };
        let resp = self
            .client
            .post(format!("{}{}", self.base_url(), path))
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Pipedrive action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
