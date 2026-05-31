//! # GitHub Connector
//!
//! Integrates with the GitHub REST API. Supports repositories, issues, pull requests,
//! organizations, teams, workflows, and releases. Supports both GitHub.com and GitHub
//! Enterprise Server via the `enterprise` constructor.
//!
//! ## Supported Objects
//! - `repositories`, `issues`, `pulls`, `orgs`, `teams`, `workflows`
//!
//! ## Supported Actions
//! - `dispatch`, `merge`, `search`
//!
//! // Dependency: `crate::connectors::common::{ApiClient, OAuth2Flow}`.
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

use crate::connectors::common::{ApiClient, OAuth2Flow};
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// GitHub connector.
///
/// Can operate against github.com or a GitHub Enterprise Server instance. Stores either
/// OAuth2 configuration or a personal access token, plus an optional enterprise base URL.
pub struct GitHubConnector {
    /// OAuth2 flow configuration.
    oauth: OAuth2Flow,
    /// Stored credentials after OAuth2 exchange or PAT setup.
    credentials: Option<Credentials>,
    /// Optional GitHub Enterprise base URL (e.g. `https://github.mycompany.com`).
    /// When `None`, the public `https://api.github.com` endpoint is used.
    enterprise_url: Option<String>,
}

impl GitHubConnector {
    /// Create a new GitHub.com connector.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
            vec![
                "repo".into(),
                "user:email".into(),
                "read:org".into(),
                "workflow".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
            enterprise_url: None,
        }
    }

    /// Create a new GitHub Enterprise connector.
    pub fn enterprise(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let base = base_url.into();
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            format!("{base}/login/oauth/authorize"),
            format!("{base}/login/oauth/access_token"),
            vec!["repo".into(), "user:email".into(), "read:org".into()],
        );
        Self {
            oauth,
            credentials: None,
            enterprise_url: Some(base),
        }
    }

    /// Create a new GitHub connector with a personal access token.
    pub fn with_api_key(token: impl Into<String>) -> Self {
        // OAuth2Flow is required by the struct even when using a PAT.
        let oauth = OAuth2Flow::new(
            "",
            "",
            "",
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
            vec!["repo".into()],
        );
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(token.into()),
                ..Default::default()
            }),
            enterprise_url: None,
        }
    }

    /// Build an authenticated [`ApiClient`] pointing to the correct base URL.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        let base = self
            .enterprise_url
            .as_ref()
            .cloned()
            .unwrap_or_else(|| "https://api.github.com".into());
        Ok(ApiClient::new(base, creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for GitHubConnector {
    fn platform_id(&self) -> &str {
        "github"
    }

    fn display_name(&self) -> &str {
        "GitHub"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::OAuth2
    }

    async fn auth_url(&self, redirect: &str) -> Result<String> {
        let mut flow = self.oauth.clone();
        flow.redirect_uri = redirect.into();
        Ok(flow.auth_url(&uuid::Uuid::new_v4().to_string()))
    }

    async fn exchange_code(&self, code: &str) -> Result<Credentials> {
        self.oauth.exchange(code).await
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let client = self.client()?;
        let limit = filters.limit.unwrap_or(30);
        // GitHub paginates with `per_page`, capped at 100.
        let per_page = limit.min(100);
        let path = match obj {
            "repositories" => "/user/repos".to_string(),
            "issues" => "/issues".to_string(),
            "pulls" => "/repos/{owner}/{repo}/pulls".to_string(),
            "orgs" => "/user/orgs".to_string(),
            "teams" => "/user/teams".to_string(),
            "workflows" => "/repos/{owner}/{repo}/actions/workflows".to_string(),
            _ => format!("/user/{obj}"),
        };
        let resp = client
            .get(&path)
            .query(&[("per_page", per_page.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitHub list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json.as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "repositories" => "/user/repos".to_string(),
            "issues" => format!(
                "/repos/{}/{}/issues",
                data.get("owner").and_then(|v| v.as_str()).unwrap_or(""),
                data.get("repo").and_then(|v| v.as_str()).unwrap_or("")
            ),
            "pulls" => format!(
                "/repos/{}/{}/pulls",
                data.get("owner").and_then(|v| v.as_str()).unwrap_or(""),
                data.get("repo").and_then(|v| v.as_str()).unwrap_or("")
            ),
            "releases" => format!(
                "/repos/{}/{}/releases",
                data.get("owner").and_then(|v| v.as_str()).unwrap_or(""),
                data.get("repo").and_then(|v| v.as_str()).unwrap_or("")
            ),
            _ => format!("/user/{obj}"),
        };
        let resp = client
            .post(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitHub create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "issues" => format!(
                "/repos/{}/{}/issues/{}",
                data.get("owner").and_then(|v| v.as_str()).unwrap_or(""),
                data.get("repo").and_then(|v| v.as_str()).unwrap_or(""),
                id
            ),
            "pulls" => format!(
                "/repos/{}/{}/pulls/{}",
                data.get("owner").and_then(|v| v.as_str()).unwrap_or(""),
                data.get("repo").and_then(|v| v.as_str()).unwrap_or(""),
                id
            ),
            _ => format!("/user/{obj}/{id}"),
        };
        let resp = client
            .patch(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitHub update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let path = match obj {
            "repositories" => format!("/repos/{id}"),
            "issues" => format!("/repos/{}/{}/issues/{}", "owner", "repo", id),
            _ => format!("/user/{obj}/{id}"),
        };
        let resp = client
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitHub delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "GitHub delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "dispatch" => format!(
                "/repos/{}/{}/actions/workflows/{}/dispatches",
                params.get("owner").and_then(|v| v.as_str()).unwrap_or(""),
                params.get("repo").and_then(|v| v.as_str()).unwrap_or(""),
                params
                    .get("workflow_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
            "merge" => format!(
                "/repos/{}/{}/pulls/{}/merge",
                params.get("owner").and_then(|v| v.as_str()).unwrap_or(""),
                params.get("repo").and_then(|v| v.as_str()).unwrap_or(""),
                params
                    .get("pull_number")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
            "search" => format!(
                "/search/{}",
                params
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("code")
            ),
            _ => format!("/{action}"),
        };
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitHub action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
