//! # Asana Connector
//!
//! Integrates with the Asana REST API (v1.0). Supports tasks, projects, workspaces,
//! teams, and users. Authentication is via a personal access token or OAuth2 Bearer token.
//!
//! ## Supported Objects
//! - `tasks`, `projects`, `workspaces`, `teams`, `users`
//!
//! ## Supported Actions
//! - `add_task_to_project`, `set_dependencies`, `search_tasks`
//!
//! // Dependency: `crate::connectors::common::ApiClient` for authenticated HTTP requests.
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

use crate::connectors::common::ApiClient;
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Asana connector.
///
/// Stores an optional [`Credentials`] containing the Bearer token. The connector is
/// constructed with a pre-known token because Asana supports personal access tokens
/// that do not require an interactive OAuth2 flow.
pub struct AsanaConnector {
    /// Bearer token credentials. Wrapped in `Option` so the connector can be
    /// instantiated before authentication in flows that require it.
    credentials: Option<Credentials>,
}

impl AsanaConnector {
    /// Create a new Asana connector with a personal access token or OAuth token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            credentials: Some(Credentials {
                access_token: Some(token.into()),
                ..Default::default()
            }),
        }
    }

    /// Build an [`ApiClient`] using the stored credentials.
    ///
    /// Returns an auth error if the connector was not initialized with credentials.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new(
            "https://app.asana.com/api/1.0",
            creds.clone(),
        ))
    }
}

#[async_trait]
impl SaaSConnector for AsanaConnector {
    fn platform_id(&self) -> &str {
        "asana"
    }

    fn display_name(&self) -> &str {
        "Asana"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::BearerToken
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        // Asana supports OAuth2 in the wild, but this implementation is optimized
        // for personal access tokens, so we reject auth_url requests.
        Err(ClawzError::Auth(
            "Asana uses bearer token authentication".into(),
        ))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth(
            "Asana uses bearer token authentication".into(),
        ))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let client = self.client()?;
        // Cap page size at 100 to respect Asana's maximum limit.
        let limit = filters.limit.unwrap_or(100).min(100);
        let path = match obj {
            "tasks" => "/tasks".to_string(),
            "projects" => "/projects".to_string(),
            "workspaces" => "/workspaces".to_string(),
            "teams" => "/teams".to_string(),
            "users" => "/users".to_string(),
            _ => format!("/{obj}"),
        };
        let resp = client
            .get(&path)
            .query(&[("limit", limit.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Asana list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Asana wraps every response in a `data` envelope.
        Ok(json["data"].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "tasks" => "/tasks",
            "projects" => "/projects",
            "sections" => "/sections",
            "attachments" => "/attachments",
            _ => return Err(ClawzError::Provider(format!("Unknown Asana object: {obj}"))),
        };
        // Asana expects top-level `data` key in request bodies.
        let body = serde_json::json!({ "data": data });
        let resp = client
            .post(path)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Asana create failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"].clone())
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "tasks" => format!("/tasks/{id}"),
            "projects" => format!("/projects/{id}"),
            "sections" => format!("/sections/{id}"),
            _ => format!("/{obj}/{id}"),
        };
        let body = serde_json::json!({ "data": data });
        let resp = client
            .put(&path)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Asana update failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"].clone())
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let path = match obj {
            "tasks" => format!("/tasks/{id}"),
            "projects" => format!("/projects/{id}"),
            _ => format!("/{obj}/{id}"),
        };
        let resp = client
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Asana delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Asana delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "add_task_to_project" => {
                let task_id = params["task_id"].as_str().unwrap_or("");
                format!("/tasks/{task_id}/addProject")
            }
            "set_dependencies" => {
                let task_id = params["task_id"].as_str().unwrap_or("");
                format!("/tasks/{task_id}/addDependencies")
            }
            "search_tasks" => "/workspaces/search".into(),
            _ => format!("/{action}"),
        };
        // Asana action endpoints also expect a `data` wrapper.
        let body = serde_json::json!({ "data": params });
        let resp = client
            .post(&path)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Asana action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
