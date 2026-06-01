//! Intercom connector for Clawz Gateway.
//!
//! Connects to the Intercom REST API (v2.10) using a bearer token. Supports
//! contacts, conversations, companies, admins, tags and notes.
//!
//! Intercom does **not** offer OAuth2 for public integrations; authentication is
//! strictly via a workspace access token.
//!
//! # Cross-module dependencies
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Intercom connector.
///
/// Holds a single bearer token and a reusable [`reqwest::Client`]. All HTTP helpers
/// (`get`, `post`, `put`, `delete`) automatically inject the `Authorization` header
/// and pin the API version to `2.10`.
pub struct IntercomConnector {
    /// Workspace access token (bearer).
    token: String,
    /// Shared HTTP client.
    client: Client,
}

impl IntercomConnector {
    /// Create a new Intercom connector with a bearer token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            client: Client::new(),
        }
    }

    /// Intercom REST API root.
    fn base_url(&self) -> &str {
        "https://api.intercom.io"
    }

    /// Start a GET request with auth and version headers already applied.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .header("Intercom-Version", "2.10")
    }

    /// Start a POST request with auth and version headers already applied.
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .header("Intercom-Version", "2.10")
    }

    /// Start a PUT request with auth and version headers already applied.
    fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .put(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .header("Intercom-Version", "2.10")
    }

    /// Start a DELETE request with auth and version headers already applied.
    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}{}", self.base_url(), path))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .header("Intercom-Version", "2.10")
    }
}

#[async_trait]
impl SaaSConnector for IntercomConnector {
    fn platform_id(&self) -> &str {
        "intercom"
    }

    fn display_name(&self) -> &str {
        "Intercom"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::BearerToken
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth(
            "Intercom uses bearer token authentication".into(),
        ))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth(
            "Intercom uses bearer token authentication".into(),
        ))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        // Intercom caps `per_page` at 150 for most endpoints.
        let per_page = filters.limit.unwrap_or(50).min(150);
        let path = match obj {
            "contacts" => format!("/contacts?per_page={per_page}"),
            "conversations" => format!("/conversations?per_page={per_page}"),
            "companies" => format!("/companies?per_page={per_page}"),
            "admins" => "/admins".to_string(),
            "tags" => "/tags".to_string(),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Intercom object: {obj}"
                )));
            }
        };
        let resp = self
            .get(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Intercom list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Intercom nests list results under "data" except for admins.
        let key = match obj {
            "admins" => "admins",
            _ => "data",
        };
        Ok(json[key].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "contacts" => "/contacts",
            "conversations" => "/conversations",
            "companies" => "/companies",
            "notes" => {
                let contact_id = data["contact_id"].as_str().unwrap_or("");
                return {
                    let resp = self
                        .post(&format!("/contacts/{contact_id}/notes"))
                        .json(&data)
                        .send()
                        .await
                        .map_err(|e| {
                            ClawzError::Provider(format!("Intercom create note failed: {e}"))
                        })?;
                    crate::connectors::common::parse_json(resp).await
                };
            }
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Intercom object: {obj}"
                )));
            }
        };
        let resp = self
            .post(path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Intercom create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "contacts" => format!("/contacts/{id}"),
            "conversations" => format!("/conversations/{id}"),
            "companies" => format!("/companies/{id}"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Intercom object: {obj}"
                )));
            }
        };
        let resp = self
            .put(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Intercom update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "contacts" => format!("/contacts/{id}"),
            "companies" => format!("/companies/{id}"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Intercom object: {obj}"
                )));
            }
        };
        let resp = self
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Intercom delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Intercom delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        match action {
            "reply_to_conversation" => {
                let conv_id = params["conversation_id"].as_str().unwrap_or("");
                let resp = self
                    .post(&format!("/conversations/{conv_id}/reply"))
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Intercom reply failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "tag_contact" => {
                let resp = self
                    .post("/contacts/tag")
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Intercom tag failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "search" => {
                let resp = self
                    .post("/contacts/search")
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Intercom search failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Intercom action: {action}"
            ))),
        }
    }
}
