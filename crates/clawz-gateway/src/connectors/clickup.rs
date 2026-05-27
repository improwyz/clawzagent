//! # ClickUp Connector
//!
//! Integrates with the ClickUp API (v2). Supports tasks, spaces, folders, lists, and
//! teams. Authentication is via a static API key sent in the `Authorization` header.
//!
//! ## Supported Objects
//! - `tasks`, `spaces`, `folders`, `lists`, `teams`
//!
//! ## Supported Actions
//! - `add_tag_to_task`, `set_custom_field`
//!
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.
//! // Dependency: `reqwest::Client` used directly for lightweight header injection.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// ClickUp connector.
///
/// Stores a single API key and a reusable reqwest client. All requests are sent to
/// `https://api.clickup.com/api/v2` with the API key attached as an `Authorization` header.
pub struct ClickUpConnector {
    /// ClickUp personal API key.
    api_key: String,
    /// Reusable async HTTP client.
    client: Client,
}

impl ClickUpConnector {
    /// Create a new ClickUp connector with an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    /// Base URL for all ClickUp v2 API calls.
    fn base_url(&self) -> &str {
        "https://api.clickup.com/api/v2"
    }

    /// Build an authenticated GET request.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.base_url(), path))
            .header("Authorization", &self.api_key)
    }

    /// Build an authenticated POST request.
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{}", self.base_url(), path))
            .header("Authorization", &self.api_key)
    }

    /// Build an authenticated PUT request.
    fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .put(format!("{}{}", self.base_url(), path))
            .header("Authorization", &self.api_key)
    }

    /// Build an authenticated DELETE request.
    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}{}", self.base_url(), path))
            .header("Authorization", &self.api_key)
    }
}

#[async_trait]
impl SaaSConnector for ClickUpConnector {
    fn platform_id(&self) -> &str {
        "clickup"
    }

    fn display_name(&self) -> &str {
        "ClickUp"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth("ClickUp uses API key authentication".into()))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth("ClickUp uses API key authentication".into()))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        // ClickUp APIs are hierarchical: tasks live inside lists, lists inside folders, etc.
        // We repurpose `filters.search` as the parent ID for navigation.
        let parent_id = filters.search.as_deref().unwrap_or("");
        let path = match obj {
            "tasks" => format!("/list/{}/task", parent_id),
            "spaces" => format!("/team/{}/space", parent_id),
            "folders" => format!("/space/{}/folder", parent_id),
            "lists" => format!("/folder/{}/list", parent_id),
            "teams" => "/team".to_string(),
            _ => return Err(ClawzError::Provider(format!("Unknown ClickUp object: {obj}"))),
        };
        let resp = self.get(&path).send().await
            .map_err(|e| ClawzError::Provider(format!("ClickUp list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[obj].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        // `parent_id` inside the payload determines the hierarchy placement.
        let parent_id = data["parent_id"].as_str().unwrap_or("");
        let path = match obj {
            "tasks" => format!("/list/{}/task", parent_id),
            "folders" => format!("/space/{}/folder", parent_id),
            "lists" => format!("/folder/{}/list", parent_id),
            _ => return Err(ClawzError::Provider(format!("Unknown ClickUp object: {obj}"))),
        };
        let resp = self.post(&path).json(&data).send().await
            .map_err(|e| ClawzError::Provider(format!("ClickUp create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "tasks" => format!("/task/{}", id),
            "folders" => format!("/folder/{}", id),
            "lists" => format!("/list/{}", id),
            _ => return Err(ClawzError::Provider(format!("Unknown ClickUp object: {obj}"))),
        };
        let resp = self.put(&path).json(&data).send().await
            .map_err(|e| ClawzError::Provider(format!("ClickUp update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "tasks" => format!("/task/{}", id),
            "folders" => format!("/folder/{}", id),
            "lists" => format!("/list/{}", id),
            _ => return Err(ClawzError::Provider(format!("Unknown ClickUp object: {obj}"))),
        };
        let resp = self.delete(&path).send().await
            .map_err(|e| ClawzError::Provider(format!("ClickUp delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "ClickUp delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let path = match action {
            "add_tag_to_task" => {
                let task_id = params["task_id"].as_str().unwrap_or("");
                let tag_name = params["tag_name"].as_str().unwrap_or("");
                format!("/task/{}/tag/{}", task_id, tag_name)
            }
            "set_custom_field" => {
                let task_id = params["task_id"].as_str().unwrap_or("");
                let field_id = params["field_id"].as_str().unwrap_or("");
                format!("/task/{}/field/{}", task_id, field_id)
            }
            _ => return Err(ClawzError::Provider(format!("Unknown ClickUp action: {action}"))),
        };
        let resp = self.post(&path).json(&params).send().await
            .map_err(|e| ClawzError::Provider(format!("ClickUp action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
