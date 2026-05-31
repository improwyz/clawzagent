//! GitLab connector for Clawz Gateway.
//!
//! This module provides a [`SaaSConnector`] implementation for GitLab (both GitLab.com
//! and self-managed instances). It supports OAuth2 authentication as well as personal
//! access tokens for server-to-server usage.
//!
//! Supported objects: projects, issues, merge_requests, pipelines, groups.
//! Supported actions: trigger_pipeline, merge_mr, create_tag.
//!
//! # Authentication
//! - OAuth2 (web flow) — recommended for user-facing integrations.
//! - Personal access token — recommended for bots / CI integrations.
//!
//! # API version
//! The connector targets the GitLab REST API v4.
//!
//! # Cross-module dependencies
//! - [`OAuth2Flow`] from `crate::connectors::common` handles the OAuth2 dance.
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait` defines the interface this
//!   connector must satisfy so the gateway can route generic requests to it.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

// Dependency: common::OAuth2Flow
use crate::connectors::common::OAuth2Flow;
// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// GitLab connector.
///
/// Holds the OAuth2 flow state, optional credentials, and the base URL of the GitLab
/// instance (GitLab.com or a self-managed domain).
pub struct GitLabConnector {
    /// OAuth2 flow configuration. Even when using token auth, a dummy [`OAuth2Flow`]
    /// is stored so the struct layout stays uniform.
    oauth: OAuth2Flow,
    /// Active credentials after a successful OAuth exchange or manual token injection.
    credentials: Option<Credentials>,
    /// Base URL of the GitLab instance, e.g. `https://gitlab.com`.
    base_url: String,
}

impl GitLabConnector {
    /// Create a new GitLab.com connector.
    ///
    /// # Arguments
    /// - `client_id`     — OAuth2 application ID.
    /// - `client_secret` — OAuth2 application secret.
    /// - `redirect_uri`  — Registered redirect URI.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self::with_base_url(client_id, client_secret, redirect_uri, "https://gitlab.com")
    }

    /// Create a GitLab self-managed connector.
    ///
    /// # Arguments
    /// - `base_url` — Root URL of the self-managed instance (no trailing `/api/v4`).
    pub fn with_base_url(
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
            format!("{base}/oauth/authorize"),
            format!("{base}/oauth/token"),
            vec!["api".into(), "read_user".into(), "read_repository".into()],
        );
        Self {
            oauth,
            credentials: None,
            base_url: base,
        }
    }

    /// Create with a personal access token.
    ///
    /// Useful for internal integrations that do not need an interactive OAuth flow.
    pub fn with_token(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        let base = base_url.into();
        // A no-op OAuth2Flow is created so the struct is fully initialised even though
        // token-based auth bypasses the OAuth machinery.
        let oauth = OAuth2Flow::new("", "", "", "", "", vec![]);
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(token.into()),
                ..Default::default()
            }),
            base_url: base,
        }
    }

    /// Build a fresh [`reqwest::Client`] for each request.
    fn client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::new())
    }

    /// Compose the v4 REST prefix from the configured base URL.
    fn api_url(&self) -> String {
        format!("{}/api/v4", self.base_url)
    }

    /// Resolve the correct Authorization header depending on what credential type is
    /// currently stored.
    ///
    /// GitLab supports two token styles:
    /// - OAuth2 access token → sent as `Authorization: Bearer <token>`.
    /// - Personal access token → sent as `PRIVATE-TOKEN: <token>`.
    fn auth_headers(&self) -> Result<Vec<(String, String)>> {
        if let Some(creds) = &self.credentials {
            if let Some(token) = &creds.access_token {
                return Ok(vec![("Authorization".into(), format!("Bearer {token}"))]);
            }
            if let Some(key) = &creds.api_key {
                return Ok(vec![("PRIVATE-TOKEN".into(), key.clone())]);
            }
        }
        Err(ClawzError::Auth("not authenticated".into()))
    }
}

#[async_trait]
impl SaaSConnector for GitLabConnector {
    fn platform_id(&self) -> &str {
        "gitlab"
    }

    fn display_name(&self) -> &str {
        "GitLab"
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
        let headers = self.auth_headers()?;
        // Cap page size at 100 because GitLab’s REST API has a hard ceiling.
        let per_page = filters.limit.unwrap_or(100).min(100);
        let path = match obj {
            "projects" => format!(
                "{}/projects?per_page={}&membership=true",
                self.api_url(),
                per_page
            ),
            "issues" => {
                let project = filters.search.as_deref().unwrap_or("");
                format!(
                    "{}/projects/{}/issues?per_page={}",
                    self.api_url(),
                    project,
                    per_page
                )
            }
            "merge_requests" => {
                let project = filters.search.as_deref().unwrap_or("");
                format!(
                    "{}/projects/{}/merge_requests?per_page={}",
                    self.api_url(),
                    project,
                    per_page
                )
            }
            "pipelines" => {
                let project = filters.search.as_deref().unwrap_or("");
                format!(
                    "{}/projects/{}/pipelines?per_page={}",
                    self.api_url(),
                    project,
                    per_page
                )
            }
            "groups" => format!(
                "{}/groups?per_page={}&min_access_level=10",
                self.api_url(),
                per_page
            ),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown GitLab object: {obj}"
                )));
            }
        };
        let mut req = client.get(&path);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitLab list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json.as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let headers = self.auth_headers()?;
        let project = data["project_id"].as_str().unwrap_or("");
        let path = match obj {
            "issues" => format!("{}/projects/{}/issues", self.api_url(), project),
            "merge_requests" => format!("{}/projects/{}/merge_requests", self.api_url(), project),
            "projects" => format!("{}/projects", self.api_url()),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown GitLab object: {obj}"
                )));
            }
        };
        let mut req = client.post(&path);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitLab create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let headers = self.auth_headers()?;
        let project = data["project_id"].as_str().unwrap_or("");
        let path = match obj {
            "issues" => format!("{}/projects/{}/issues/{}", self.api_url(), project, id),
            "merge_requests" => format!(
                "{}/projects/{}/merge_requests/{}",
                self.api_url(),
                project,
                id
            ),
            "projects" => format!("{}/projects/{}", self.api_url(), id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown GitLab object: {obj}"
                )));
            }
        };
        let mut req = client.put(&path);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitLab update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let headers = self.auth_headers()?;
        let path = match obj {
            "projects" => format!("{}/projects/{}", self.api_url(), id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown GitLab object: {obj}"
                )));
            }
        };
        let mut req = client.delete(&path);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitLab delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "GitLab delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let headers = self.auth_headers()?;
        let project = params["project_id"].as_str().unwrap_or("");
        let path = match action {
            "trigger_pipeline" => format!("{}/projects/{}/pipeline", self.api_url(), project),
            "merge_mr" => {
                let mr_iid = params["mr_iid"].as_str().unwrap_or("");
                format!(
                    "{}/projects/{}/merge_requests/{}/merge",
                    self.api_url(),
                    project,
                    mr_iid
                )
            }
            "create_tag" => format!("{}/projects/{}/repository/tags", self.api_url(), project),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown GitLab action: {action}"
                )));
            }
        };
        let mut req = client.post(&path);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GitLab action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
