//! # Atlassian Connector
//!
//! Integrates with Atlassian Cloud APIs: Jira (issue tracking, agile boards) and
//! Confluence (pages). Supports both OAuth2 and API-token authentication.
//!
//! ## Supported Objects
//! - `issues`, `projects`, `boards`, `pages`
//!
//! ## Supported Actions
//! - `transition`, `search`, `myself`
//!
//! // Dependency: `crate::connectors::common::{ApiClient, OAuth2Flow}`.
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

use crate::connectors::common::{ApiClient, OAuth2Flow};
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Atlassian connector (Jira, Confluence).
///
/// Holds either an OAuth2 flow configuration or an API token, plus the base URL of the
/// Atlassian Cloud instance. The base URL is required because API-token auth uses Basic
/// auth against the instance rather than Atlassian's central OAuth2 servers.
pub struct AtlassianConnector {
    /// OAuth2 flow state machine. Populated even for API-key mode because `ApiClient`
    /// is not used directly for API-key auth; instead the token is stored in `credentials`.
    oauth: OAuth2Flow,
    /// Stored credentials after OAuth2 exchange or API-key setup.
    credentials: Option<Credentials>,
    /// Base URL of the Atlassian instance, e.g. `https://mycompany.atlassian.net`.
    base_url: String,
}

impl AtlassianConnector {
    /// Create a new Atlassian connector configured for OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://auth.atlassian.com/authorize",
            "https://auth.atlassian.com/oauth/token",
            vec![
                "read:jira-work".into(),
                "write:jira-work".into(),
                "read:confluence-content.summary".into(),
                "offline_access".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
            base_url: base_url.into(),
        }
    }

    /// Create a new Atlassian connector with an API token.
    ///
    /// The API token is stored as `api_key` inside [`Credentials`] and sent via Basic auth.
    pub fn with_api_key(api_token: impl Into<String>, base_url: impl Into<String>) -> Self {
        // Dummy OAuth2Flow is needed because the struct requires it even in API-key mode.
        // It is never used for actual token exchange in this path.
        let oauth = OAuth2Flow::new(
            "",
            "",
            "",
            "https://auth.atlassian.com/authorize",
            "https://auth.atlassian.com/oauth/token",
            vec!["read:jira-work".into()],
        );
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(api_token.into()),
                ..Default::default()
            }),
            base_url: base_url.into(),
        }
    }

    /// Build an authenticated [`ApiClient`] for the configured base URL.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new(self.base_url.clone(), creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for AtlassianConnector {
    fn platform_id(&self) -> &str {
        "atlassian"
    }

    fn display_name(&self) -> &str {
        "Atlassian"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::OAuth2
    }

    async fn auth_url(&self, redirect: &str) -> Result<String> {
        // Clone the flow so we can temporarily override the redirect URI per-request.
        let mut flow = self.oauth.clone();
        flow.redirect_uri = redirect.into();
        Ok(flow.auth_url(&uuid::Uuid::new_v4().to_string()))
    }

    async fn exchange_code(&self, code: &str) -> Result<Credentials> {
        self.oauth.exchange(code).await
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let client = self.client()?;
        let limit = filters.limit.unwrap_or(50);
        let path = match obj {
            "issues" => format!("/rest/api/3/search?maxResults={}", limit),
            "projects" => format!("/rest/api/3/project?maxResults={}", limit),
            "boards" => format!("/rest/agile/1.0/board?maxResults={}", limit),
            "pages" => format!("/wiki/rest/api/content?limit={}", limit),
            _ => format!("/rest/api/3/{}", obj),
        };
        let resp = client
            .get(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Atlassian list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Atlassian APIs do not use a single response key; map per-object type.
        let key = match obj {
            "issues" => "issues",
            "projects" => "values",
            "boards" => "values",
            "pages" => "results",
            _ => "values",
        };
        Ok(json
            .get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "issues" => "/rest/api/3/issue".to_string(),
            "pages" => "/wiki/rest/api/content".to_string(),
            _ => format!("/rest/api/3/{}", obj),
        };
        let resp = client
            .post(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Atlassian create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "issues" => format!("/rest/api/3/issue/{}", id),
            "pages" => format!("/wiki/rest/api/content/{}", id),
            _ => format!("/rest/api/3/{}/{}", obj, id),
        };
        let resp = client
            .put(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Atlassian update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let path = match obj {
            "issues" => format!("/rest/api/3/issue/{}", id),
            "pages" => format!("/wiki/rest/api/content/{}", id),
            _ => format!("/rest/api/3/{}/{}", obj, id),
        };
        let resp = client
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Atlassian delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Atlassian delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "transition" => format!(
                "/rest/api/3/issue/{}/transitions",
                params.get("issueId").and_then(|v| v.as_str()).unwrap_or("")
            ),
            "search" => "/rest/api/3/search".to_string(),
            "myself" => "/rest/api/3/myself".to_string(),
            _ => format!("/rest/api/3/{}", action),
        };
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Atlassian action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
