//! Slack workspace connector for Clawz Gateway.
//!
//! Integrates with the Slack Web API. Supports channels, users, files, messages
//! and various conversational actions.
//!
//! # Authentication
//! - OAuth2 (user-facing app installation).
//! - Bot token (`xoxb-...`) injected directly for single-workspace bots.
//!
//! # Important Slack-isms
//! - Slack uses method-name URLs (`/conversations.list`, `chat.postMessage`, …)
//!   rather than REST resource paths.
//! - Every JSON response contains an `ok` boolean that must be checked; HTTP 200
//!   does **not** guarantee success.
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

/// Slack workspace connector.
///
/// All requests target `https://slack.com/api`. The connector maps generic object
/// names to Slack Web API method names.
pub struct SlackConnector {
    /// OAuth2 flow configuration for Slack’s v2 auth flow.
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange, or a manually injected bot token.
    credentials: Option<Credentials>,
}

impl SlackConnector {
    /// Create a new Slack connector.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://slack.com/oauth/v2/authorize",
            "https://slack.com/api/oauth.v2.access",
            vec![
                "channels:read".into(),
                "groups:read".into(),
                "chat:write".into(),
                "users:read".into(),
                "files:read".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
        }
    }

    /// Create a new Slack connector with a bot token.
    ///
    /// Useful for single-workspace bots that do not need an interactive OAuth flow.
    pub fn with_api_key(bot_token: impl Into<String>) -> Self {
        let oauth = OAuth2Flow::new(
            "",
            "",
            "",
            "https://slack.com/oauth/v2/authorize",
            "https://slack.com/api/oauth.v2.access",
            vec!["channels:read".into()],
        );
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(bot_token.into()),
                ..Default::default()
            }),
        }
    }

    /// Build an [`ApiClient`] against the Slack Web API base URL.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new("https://slack.com/api", creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for SlackConnector {
    fn platform_id(&self) -> &str {
        "slack"
    }

    fn display_name(&self) -> &str {
        "Slack"
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
        // Slack object names map directly to Web API method names.
        let method = match obj {
            "channels" => "conversations.list",
            "users" => "users.list",
            "files" => "files.list",
            "messages" => "conversations.history",
            _ => obj,
        };
        let limit = filters.limit.unwrap_or(100);
        let resp = client
            .post(&format!("/{}", method))
            .form(&[("limit", limit.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Slack list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Slack returns HTTP 200 even for errors; we must inspect the `ok` field.
        if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(ClawzError::Provider(format!(
                "Slack API error: {}",
                json.get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            )));
        }
        // Slack uses different top-level keys for different method responses.
        let key = match obj {
            "channels" => "channels",
            "users" => "members",
            "files" => "files",
            "messages" => "messages",
            _ => obj,
        };
        Ok(json
            .get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let method = match obj {
            "messages" => "chat.postMessage",
            "channels" => "conversations.create",
            _ => obj,
        };
        let resp = client
            .post(&format!("/{}", method))
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Slack create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let method = match obj {
            "messages" => "chat.update",
            _ => obj,
        };
        // Slack message updates require the message timestamp (`ts`) in the payload.
        let mut payload = data.clone();
        if let Some(map) = payload.as_object_mut() {
            map.insert("ts".into(), id.into());
        }
        let resp = client
            .post(&format!("/{}", method))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Slack update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let method = match obj {
            "messages" => "chat.delete",
            "files" => "files.delete",
            _ => obj,
        };
        let resp = client
            .post(&format!("/{}", method))
            .form(&[("ts", id), ("channel", "")])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Slack delete failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        if json.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Slack delete failed: {}",
                json.get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let method = match action {
            "postMessage" => "chat.postMessage",
            "postEphemeral" => "chat.postEphemeral",
            "invite" => "conversations.invite",
            "setTopic" => "conversations.setTopic",
            "archive" => "conversations.archive",
            _ => action,
        };
        let resp = client
            .post(&format!("/{}", method))
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Slack action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
