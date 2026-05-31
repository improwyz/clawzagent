//! Microsoft 365 connector for Clawz Gateway.
//!
//! Wraps the Microsoft Graph API v1.0 and supports Outlook mail, calendar events,
//! OneDrive files, Teams messages and directory users.
//!
//! # Authentication
//! OAuth2 against a specific Azure AD tenant. The `tenant_id` controls which
//! organisation’s login endpoint is used (`common`, `consumers`, or a GUID).
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

/// Microsoft 365 connector (Outlook, OneDrive, Teams, Graph API).
///
/// All operations are routed through `https://graph.microsoft.com/v1.0`.
/// The tenant ID is baked into the OAuth2 authorize/token URLs so users authenticate
/// against the correct Azure AD instance.
/// Microsoft 365 connector — email, calendar, SharePoint, Teams integration.
#[allow(dead_code)]
pub struct Microsoft365Connector {
    /// OAuth2 flow scoped to the tenant’s endpoints.
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange.
    credentials: Option<Credentials>,
    /// Azure AD tenant identifier (e.g. `common`, `consumers`, or a GUID).
    tenant_id: String,
}

impl Microsoft365Connector {
    /// Create a new Microsoft 365 connector.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        tenant_id: impl Into<String>,
    ) -> Self {
        let tenant = tenant_id.into();
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
            format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
            vec![
                "https://graph.microsoft.com/User.Read".into(),
                "https://graph.microsoft.com/Mail.Read".into(),
                "https://graph.microsoft.com/Files.ReadWrite".into(),
                "offline_access".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
            tenant_id: tenant,
        }
    }

    /// Build an [`ApiClient`] targeting the Microsoft Graph v1.0 endpoint.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new(
            "https://graph.microsoft.com/v1.0",
            creds.clone(),
        ))
    }
}

#[async_trait]
impl SaaSConnector for Microsoft365Connector {
    fn platform_id(&self) -> &str {
        "microsoft365"
    }

    fn display_name(&self) -> &str {
        "Microsoft 365"
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
        let limit = filters.limit.unwrap_or(100);
        let path = match obj {
            "users" => "/users".to_string(),
            "messages" => "/me/messages".to_string(),
            "files" => "/me/drive/root/children".to_string(),
            "events" => "/me/events".to_string(),
            "teams" => "/me/joinedTeams".to_string(),
            _ => format!("/me/{obj}?top={limit}"),
        };
        let resp = client
            .get(&path)
            .query(&[("$top", limit.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Microsoft365 list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Microsoft Graph wraps collections in a `value` array.
        Ok(json
            .get("value")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "messages" => "/me/messages".to_string(),
            "events" => "/me/events".to_string(),
            "files" => "/me/drive/root/children".to_string(),
            _ => format!("/me/{obj}"),
        };
        let resp = client
            .post(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Microsoft365 create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "messages" => format!("/me/messages/{id}"),
            "events" => format!("/me/events/{id}"),
            "files" => format!("/me/drive/items/{id}"),
            _ => format!("/me/{obj}/{id}"),
        };
        let resp = client
            .patch(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Microsoft365 update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let path = match obj {
            "messages" => format!("/me/messages/{id}"),
            "events" => format!("/me/events/{id}"),
            "files" => format!("/me/drive/items/{id}"),
            _ => format!("/me/{obj}/{id}"),
        };
        let resp = client
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Microsoft365 delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Microsoft365 delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "sendMail" => "/me/sendMail".to_string(),
            "sendMessage" => format!(
                "/teams/{}/channels/{}/messages",
                params.get("teamId").and_then(|v| v.as_str()).unwrap_or(""),
                params
                    .get("channelId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
            "search" => "/me/microsoft.graph.search".to_string(),
            _ => format!("/me/{action}"),
        };
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Microsoft365 action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
