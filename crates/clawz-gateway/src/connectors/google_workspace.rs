//! Google Workspace connector for Clawz Gateway.
//!
//! Provides unified access to Gmail, Google Drive, Calendar, Sheets and the Admin
//! Directory via Google’s OAuth2 flow or a service-account-style API key.
//!
//! # Supported objects
//! - `messages` (Gmail)
//! - `threads` (Gmail)
//! - `files` (Drive)
//! - `events` (Calendar)
//! - `spreadsheets` (Sheets)
//! - `users` (Admin SDK)
//!
//! # Cross-module dependencies
//! - [`OAuth2Flow`] and [`ApiClient`] from `crate::connectors::common`.
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

// Dependency: common::{ApiClient, OAuth2Flow}
use crate::connectors::common::{ApiClient, OAuth2Flow};
// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Google Workspace connector (Gmail, Drive, Calendar, Sheets).
///
/// Each object type is mapped to a different Google API subdomain (gmail, www, sheets,
/// admin). The connector uses [`ApiClient`] to centralise bearer-token injection.
pub struct GoogleWorkspaceConnector {
    /// OAuth2 flow state for Google’s auth and token endpoints.
    oauth: OAuth2Flow,
    /// Active credentials after a successful OAuth exchange.
    credentials: Option<Credentials>,
}

impl GoogleWorkspaceConnector {
    /// Create a new Google Workspace connector.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            vec![
                "https://www.googleapis.com/auth/gmail.readonly".into(),
                "https://www.googleapis.com/auth/drive".into(),
                "https://www.googleapis.com/auth/calendar".into(),
                "https://www.googleapis.com/auth/spreadsheets".into(),
                "https://www.googleapis.com/auth/admin.directory.user.readonly".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
        }
    }

    /// Create a new Google Workspace connector with a service account key.
    ///
    /// This is mainly a convenience constructor for internal / daemon usage. The
    /// `api_key` is stored in `Credentials::api_key` and forwarded as an
    /// `Authorization: Bearer` header by [`ApiClient`].
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        let oauth = OAuth2Flow::new(
            "",
            "",
            "",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            vec!["https://www.googleapis.com/auth/cloud-platform".into()],
        );
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(api_key.into()),
                ..Default::default()
            }),
        }
    }

    /// Build an [`ApiClient`] pinned to the correct Google API base URL.
    ///
    /// The `api` parameter is the subdomain portion of `https://{api}.googleapis.com`.
    fn client(&self, api: &str) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        let base = format!("https://{api}.googleapis.com");
        Ok(ApiClient::new(base, creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for GoogleWorkspaceConnector {
    fn platform_id(&self) -> &str {
        "google_workspace"
    }

    fn display_name(&self) -> &str {
        "Google Workspace"
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
        // Map each object type to the Google API subdomain and REST path.
        let (api, path) = match obj {
            "messages" => ("gmail", "/gmail/v1/users/me/messages".to_string()),
            "threads" => ("gmail", "/gmail/v1/users/me/threads".to_string()),
            "files" => ("www", "/drive/v3/files".to_string()),
            "events" => ("www", "/calendar/v3/calendars/primary/events".to_string()),
            "spreadsheets" => ("sheets", "/sheets/v4/spreadsheets".to_string()),
            "users" => ("admin", "/admin/directory/v1/users".to_string()),
            _ => ("www", format!("/{obj}?alt=json")),
        };
        let client = self.client(api)?;
        let limit = filters.limit.unwrap_or(100);
        let resp = client
            .get(&path)
            .query(&[("maxResults", limit.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GoogleWorkspace list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Google APIs use inconsistent top-level keys for collections.
        let key = match obj {
            "messages" | "threads" => "messages",
            "files" => "files",
            "events" => "items",
            "spreadsheets" => "spreadsheets",
            "users" => "users",
            _ => "items",
        };
        Ok(json
            .get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let (api, path) = match obj {
            "messages" => ("gmail", "/gmail/v1/users/me/messages".to_string()),
            "files" => (
                "www",
                "/upload/drive/v3/files?uploadType=multipart".to_string(),
            ),
            "events" => ("www", "/calendar/v3/calendars/primary/events".to_string()),
            "spreadsheets" => ("sheets", "/sheets/v4/spreadsheets".to_string()),
            _ => ("www", format!("/{obj}?alt=json")),
        };
        let client = self.client(api)?;
        let resp = client
            .post(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GoogleWorkspace create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let (api, path) = match obj {
            "messages" => ("gmail", format!("/gmail/v1/users/me/messages/{id}")),
            "files" => ("www", format!("/drive/v3/files/{id}")),
            "events" => ("www", format!("/calendar/v3/calendars/primary/events/{id}")),
            "spreadsheets" => ("sheets", format!("/sheets/v4/spreadsheets/{id}")),
            _ => ("www", format!("/me/{obj}/{id}")),
        };
        let client = self.client(api)?;
        let resp = client
            .put(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GoogleWorkspace update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let (api, path) = match obj {
            "messages" => ("gmail", format!("/gmail/v1/users/me/messages/{id}")),
            "files" => ("www", format!("/drive/v3/files/{id}")),
            "events" => ("www", format!("/calendar/v3/calendars/primary/events/{id}")),
            "spreadsheets" => ("sheets", format!("/sheets/v4/spreadsheets/{id}")),
            _ => ("www", format!("/me/{obj}/{id}")),
        };
        let client = self.client(api)?;
        let resp = client
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GoogleWorkspace delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "GoogleWorkspace delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let (api, path) = match action {
            "send" => ("gmail", "/gmail/v1/users/me/messages/send".to_string()),
            "sendMessage" => (
                "www",
                format!(
                    "/teams/{}/channels/{}/messages",
                    params.get("teamId").and_then(|v| v.as_str()).unwrap_or(""),
                    params
                        .get("channelId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                ),
            ),
            "watch" => (
                "www",
                "/calendar/v3/calendars/primary/events/watch".to_string(),
            ),
            _ => ("www", format!("/me/{action}?alt=json")),
        };
        let client = self.client(api)?;
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("GoogleWorkspace action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
