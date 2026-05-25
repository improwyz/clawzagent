//! Notion connector for Clawz Gateway.
//!
//! Talks to the Notion REST API v1. Supports OAuth2 (public integrations) and
//! internal integration tokens.
//!
//! # Supported objects
//! - `pages`, `databases`, `blocks`
//!
//! # Supported actions
//! - `query_database`, `search`
//!
//! # Cross-module dependencies
//! - [`OAuth2Flow`] from `crate::connectors::common`.
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

// Dependency: common::OAuth2Flow
use crate::connectors::common::OAuth2Flow;
// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Notion API version pinned by this connector.
///
/// Sent as the `Notion-Version` header on every request.
const NOTION_VERSION: &str = "2022-06-28";

/// Notion connector.
///
/// Handles both OAuth2 public integrations and internal (token-only) integrations.
/// The `Notion-Version` header is mandatory and strictly enforced by the API.
pub struct NotionConnector {
    /// OAuth2 flow configuration (active for public integrations).
    oauth: OAuth2Flow,
    /// Credentials after OAuth exchange, or a manually injected internal token.
    credentials: Option<Credentials>,
}

impl NotionConnector {
    /// Create a new Notion connector via OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://api.notion.com/v1/oauth/authorize",
            "https://api.notion.com/v1/oauth/token",
            vec!["read_content".into(), "update_content".into(), "insert_content".into()],
        );
        Self { oauth, credentials: None }
    }

    /// Create with an integration token (internal integrations).
    ///
    /// Internal integrations have a fixed scope and do not require an OAuth flow.
    pub fn with_token(token: impl Into<String>) -> Self {
        let oauth = OAuth2Flow::new("", "", "", "", "", vec![]);
        Self {
            oauth,
            credentials: Some(Credentials {
                access_token: Some(token.into()),
                ..Default::default()
            }),
        }
    }

    fn client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::new())
    }

    /// Extract the bearer token from stored credentials.
    fn token(&self) -> Result<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.clone())
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))
    }

    fn base_url(&self) -> &str {
        "https://api.notion.com/v1"
    }
}

#[async_trait]
impl SaaSConnector for NotionConnector {
    fn platform_id(&self) -> &str {
        "notion"
    }

    fn display_name(&self) -> &str {
        "Notion"
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
        let token = self.token()?;
        // Notion caps page_size at 100.
        let page_size = filters.limit.unwrap_or(100).min(100);

        match obj {
            "pages" | "databases" => {
                // Use the search endpoint with an object-type filter.
                let body = serde_json::json!({
                    "filter": { "value": obj.trim_end_matches('s'), "property": "object" },
                    "page_size": page_size
                });
                let resp = client
                    .post(format!("{}/search", self.base_url()))
                    .bearer_auth(&token)
                    .header("Notion-Version", NOTION_VERSION)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Notion list failed: {e}")))?;
                let json: Value = crate::connectors::common::parse_json(resp).await?;
                Ok(json["results"].as_array().cloned().unwrap_or_default())
            }
            "blocks" => {
                let block_id = filters.search.as_deref().unwrap_or("");
                let resp = client
                    .get(format!("{}/blocks/{}/children", self.base_url(), block_id))
                    .bearer_auth(&token)
                    .header("Notion-Version", NOTION_VERSION)
                    .query(&[("page_size", page_size.to_string())])
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Notion list blocks failed: {e}")))?;
                let json: Value = crate::connectors::common::parse_json(resp).await?;
                Ok(json["results"].as_array().cloned().unwrap_or_default())
            }
            _ => {
                // Fallback to a generic search when the object type is not explicitly handled.
                let body = serde_json::json!({ "page_size": page_size });
                let resp = client
                    .post(format!("{}/search", self.base_url()))
                    .bearer_auth(&token)
                    .header("Notion-Version", NOTION_VERSION)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Notion search failed: {e}")))?;
                let json: Value = crate::connectors::common::parse_json(resp).await?;
                Ok(json["results"].as_array().cloned().unwrap_or_default())
            }
        }
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let token = self.token()?;
        let path = match obj {
            "pages" => "/pages",
            "databases" => "/databases",
            "blocks" => {
                let block_id = data["block_id"].as_str().unwrap_or("");
                return {
                    let resp = client
                        .patch(format!("{}/blocks/{}/children", self.base_url(), block_id))
                        .bearer_auth(&token)
                        .header("Notion-Version", NOTION_VERSION)
                        .json(&data)
                        .send()
                        .await
                        .map_err(|e| ClawzError::Provider(format!("Notion create block failed: {e}")))?;
                    crate::connectors::common::parse_json(resp).await
                };
            }
            _ => "/pages",
        };
        let resp = client
            .post(format!("{}{}", self.base_url(), path))
            .bearer_auth(&token)
            .header("Notion-Version", NOTION_VERSION)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Notion create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let token = self.token()?;
        let path = match obj {
            "pages" => format!("/pages/{}", id),
            "databases" => format!("/databases/{}", id),
            "blocks" => format!("/blocks/{}", id),
            _ => format!("/pages/{}", id),
        };
        let resp = client
            .patch(format!("{}{}", self.base_url(), path))
            .bearer_auth(&token)
            .header("Notion-Version", NOTION_VERSION)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Notion update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let token = self.token()?;
        let path = match obj {
            "blocks" => format!("/blocks/{}", id),
            _ => format!("/pages/{}", id),
        };
        let resp = client
            .delete(format!("{}{}", self.base_url(), path))
            .bearer_auth(&token)
            .header("Notion-Version", NOTION_VERSION)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Notion delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Notion delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let token = self.token()?;
        let path = match action {
            "query_database" => {
                let db_id = params["database_id"].as_str().unwrap_or("");
                format!("/databases/{}/query", db_id)
            }
            "search" => "/search".into(),
            _ => format!("/{}", action),
        };
        let resp = client
            .post(format!("{}{}", self.base_url(), path))
            .bearer_auth(&token)
            .header("Notion-Version", NOTION_VERSION)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Notion action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
