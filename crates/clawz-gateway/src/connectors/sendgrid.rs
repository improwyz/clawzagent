//! SendGrid connector for Clawz Gateway.
//!
//! Integrates with the SendGrid v3 REST API. Supports transactional email,
//! marketing contacts, dynamic templates and sender profiles.
//!
//! # Authentication
//! API key only (bearer token). SendGrid OAuth2 is not supported here.
//!
//! # Cross-module dependencies
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// SendGrid connector.
///
/// All requests target `https://api.sendgrid.com/v3`. The connector pre-configures
/// `Authorization: Bearer <api_key>` on every request builder.
pub struct SendGridConnector {
    /// SendGrid API key (bearer token).
    api_key: String,
    /// Shared HTTP client.
    client: Client,
}

impl SendGridConnector {
    /// Create a new SendGrid connector with an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    /// SendGrid v3 API root.
    fn base_url(&self) -> &str {
        "https://api.sendgrid.com/v3"
    }

    /// Start a GET request with bearer auth already applied.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.api_key)
    }

    /// Start a POST request with bearer auth already applied.
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.api_key)
    }

    /// Start a PATCH request with bearer auth already applied.
    fn patch(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .patch(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.api_key)
    }

    /// Start a DELETE request with bearer auth already applied.
    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.api_key)
    }
}

#[async_trait]
impl SaaSConnector for SendGridConnector {
    fn platform_id(&self) -> &str {
        "sendgrid"
    }

    fn display_name(&self) -> &str {
        "SendGrid"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth(
            "SendGrid uses API key authentication".into(),
        ))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth(
            "SendGrid uses API key authentication".into(),
        ))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let limit = filters.limit.unwrap_or(100).min(500);
        let path = match obj {
            "templates" => format!("/templates?generations=dynamic&page_size={limit}"),
            "contacts" => format!("/marketing/contacts?page_size={limit}"),
            "lists" => "/marketing/lists".to_string(),
            "segments" => "/marketing/segments/2.0".to_string(),
            "senders" => "/marketing/senders".to_string(),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown SendGrid object: {obj}"
                )));
            }
        };
        let resp = self
            .get(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("SendGrid list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // SendGrid uses inconsistent top-level keys for collections.
        let key = match obj {
            "templates" => "templates",
            "contacts" => "result",
            "lists" => "result",
            "segments" => "results",
            "senders" => "result",
            _ => "result",
        };
        Ok(json[key]
            .as_array()
            .cloned()
            .unwrap_or_else(|| json.as_array().cloned().unwrap_or_default()))
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "email" => "/mail/send",
            "templates" => "/templates",
            "lists" => "/marketing/lists",
            "contacts" => "/marketing/contacts",
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown SendGrid object: {obj}"
                )));
            }
        };
        let resp = self
            .post(path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("SendGrid create failed: {e}")))?;
        // SendGrid returns 202 Accepted for successful mail send requests.
        if resp.status().as_u16() == 202 {
            Ok(serde_json::json!({ "status": "accepted" }))
        } else {
            crate::connectors::common::parse_json(resp).await
        }
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "templates" => format!("/templates/{id}"),
            "lists" => format!("/marketing/lists/{id}"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown SendGrid object: {obj}"
                )));
            }
        };
        let resp = self
            .patch(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("SendGrid update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "templates" => format!("/templates/{id}"),
            "lists" => format!("/marketing/lists/{id}"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown SendGrid object: {obj}"
                )));
            }
        };
        let resp = self
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("SendGrid delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "SendGrid delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let path = match action {
            "send_email" => "/mail/send",
            "validate_email" => "/validations/email",
            "send_batch" => "/mail/batch",
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown SendGrid action: {action}"
                )));
            }
        };
        let resp = self
            .post(path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("SendGrid action failed: {e}")))?;
        if resp.status().as_u16() == 202 {
            Ok(serde_json::json!({ "status": "accepted" }))
        } else {
            crate::connectors::common::parse_json(resp).await
        }
    }
}
