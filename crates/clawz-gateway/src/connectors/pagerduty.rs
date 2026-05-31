//! PagerDuty connector for Clawz Gateway.
//!
//! Provides incident management, on-call scheduling and escalation-policy access
//! via the PagerDuty REST API v2.
//!
//! # Authentication
//! API key only (token-based). PagerDuty OAuth2 is available but not implemented
//! in this connector.
//!
//! # Cross-module dependencies
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// PagerDuty connector.
///
/// Uses a PagerDuty API v2 key and the `Authorization: Token token=<key>` header
/// style. All request builders automatically set the `Accept` version header.
pub struct PagerDutyConnector {
    /// PagerDuty API v2 key.
    api_key: String,
    /// Shared HTTP client.
    client: Client,
}

impl PagerDutyConnector {
    /// Create a new PagerDuty connector with an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    /// PagerDuty REST API root.
    fn base_url(&self) -> &str {
        "https://api.pagerduty.com"
    }

    /// Start a GET request with the PagerDuty auth and version headers.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .header("Accept", "application/vnd.pagerduty+json;version=2")
    }

    /// Start a POST request with the PagerDuty auth and version headers.
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .header("Accept", "application/vnd.pagerduty+json;version=2")
    }

    /// Start a PUT request with the PagerDuty auth and version headers.
    fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .put(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .header("Accept", "application/vnd.pagerduty+json;version=2")
    }

    /// Start a DELETE request with the PagerDuty auth and version headers.
    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .header("Accept", "application/vnd.pagerduty+json;version=2")
    }
}

#[async_trait]
impl SaaSConnector for PagerDutyConnector {
    fn platform_id(&self) -> &str {
        "pagerduty"
    }

    fn display_name(&self) -> &str {
        "PagerDuty"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth(
            "PagerDuty uses API key authentication".into(),
        ))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth(
            "PagerDuty uses API key authentication".into(),
        ))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let limit = filters.limit.unwrap_or(100).min(100);
        let path = match obj {
            "incidents" => format!("/incidents?limit={limit}"),
            "services" => format!("/services?limit={limit}"),
            "escalation_policies" => format!("/escalation_policies?limit={limit}"),
            "schedules" => format!("/schedules?limit={limit}"),
            "users" => format!("/users?limit={limit}"),
            "teams" => format!("/teams?limit={limit}"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown PagerDuty object: {obj}"
                )));
            }
        };
        let resp = self
            .get(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("PagerDuty list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[obj].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let (path, key) = match obj {
            "incidents" => ("/incidents", "incident"),
            "services" => ("/services", "service"),
            "escalation_policies" => ("/escalation_policies", "escalation_policy"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown PagerDuty object: {obj}"
                )));
            }
        };
        // PagerDuty expects a wrapper object (e.g. `{ "incident": { ... } }`).
        let body = serde_json::json!({ key: data });
        let resp = self
            .post(path)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("PagerDuty create failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[key].clone())
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let (path, key) = match obj {
            "incidents" => (format!("/incidents/{id}"), "incident"),
            "services" => (format!("/services/{id}"), "service"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown PagerDuty object: {obj}"
                )));
            }
        };
        let body = serde_json::json!({ key: data });
        let resp = self
            .put(&path)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("PagerDuty update failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[key].clone())
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "services" => format!("/services/{id}"),
            "escalation_policies" => format!("/escalation_policies/{id}"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Cannot delete PagerDuty {obj}"
                )));
            }
        };
        let resp = self
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("PagerDuty delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "PagerDuty delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        match action {
            "acknowledge_incident" | "resolve_incident" => {
                let id = params["id"].as_str().unwrap_or("");
                let status = if action == "acknowledge_incident" {
                    "acknowledged"
                } else {
                    "resolved"
                };
                let body = serde_json::json!({
                    "incident": { "type": "incident_reference", "status": status }
                });
                let resp = self
                    .put(&format!("/incidents/{id}"))
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("PagerDuty {action} failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "create_override" => {
                let schedule_id = params["schedule_id"].as_str().unwrap_or("");
                let body = serde_json::json!({ "override": params });
                let resp = self
                    .post(&format!("/schedules/{schedule_id}/overrides"))
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("PagerDuty create override failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown PagerDuty action: {action}"
            ))),
        }
    }
}
