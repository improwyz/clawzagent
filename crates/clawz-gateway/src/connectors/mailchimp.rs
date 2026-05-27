//! Mailchimp connector for Clawz Gateway.
//!
//! Integrates with the Mailchimp Marketing API v3.0. Supports both OAuth2 and
//! API-key authentication. The API key format is `key-dcXX` where `dcXX` is the
//! data-center identifier (e.g. `us1`).
//!
//! # Supported objects
//! - `lists`, `campaigns`, `templates`, `members`
//!
//! # Supported actions
//! - `send_campaign`, `schedule_campaign`, `add_member_to_list`
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

/// Mailchimp connector.
///
/// The connector needs to know the data-center because Mailchimp shards accounts
/// across subdomains (`us1.api.mailchimp.com`, `eu1.api.mailchimp.com`, etc.).
/// When using OAuth2 the data-center is discovered lazily; when using an API key
/// it is parsed from the suffix of the key.
pub struct MailchimpConnector {
    /// OAuth2 flow configuration (active for web-flow integrations).
    oauth: OAuth2Flow,
    /// Credentials after OAuth exchange, or a manually injected API key.
    credentials: Option<Credentials>,
    /// Parsed data-center identifier (e.g. `us1`).
    data_center: Option<String>,
}

impl MailchimpConnector {
    /// Create a new Mailchimp connector via OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://login.mailchimp.com/oauth2/authorize",
            "https://login.mailchimp.com/oauth2/token",
            vec![],
        );
        Self {
            oauth,
            credentials: None,
            data_center: None,
        }
    }

    /// Create with an API key (format: key-dcXX).
    ///
    /// The data-center suffix is extracted so the correct API subdomain can be used
    /// for every request.
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        let key: String = api_key.into();
        // Extract data center from key (e.g., "abc123-us1" -> "us1")
        let dc = key.rsplit('-').next().map(|s| s.to_string());
        let oauth = OAuth2Flow::new("", "", "", "", "", vec![]);
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(key),
                ..Default::default()
            }),
            data_center: dc,
        }
    }

    /// Build the per-data-center API base URL.
    fn base_url(&self) -> String {
        let dc = self.data_center.as_deref().unwrap_or("us1");
        format!("https://{}.api.mailchimp.com/3.0", dc)
    }

    fn client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::new())
    }

    /// Resolve the Authorization header.
    ///
    /// Mailchimp supports two styles:
    /// - OAuth2 → `Authorization: Bearer <token>`.
    /// - API key  → `Authorization: Basic <base64(anystring:key)>`.
    fn auth_header(&self) -> String {
        if let Some(creds) = &self.credentials {
            if let Some(token) = &creds.access_token {
                return format!("Bearer {}", token);
            }
            if let Some(key) = &creds.api_key {
                // Mailchimp API key auth uses Basic: anystring:key
                let encoded = base64_encode(&format!("anystring:{}", key));
                return format!("Basic {}", encoded);
            }
        }
        String::new()
    }
}

/// Small helper because the base64 crate API is verbose.
fn base64_encode(s: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

#[async_trait]
impl SaaSConnector for MailchimpConnector {
    fn platform_id(&self) -> &str {
        "mailchimp"
    }

    fn display_name(&self) -> &str {
        "Mailchimp"
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
        // Mailchimp allows up to 1000 items per page for lists.
        let count = filters.limit.unwrap_or(100).min(1000);
        let path = match obj {
            "lists" => "/lists".to_string(),
            "campaigns" => "/campaigns".to_string(),
            "templates" => "/templates".to_string(),
            "members" => {
                let list_id = filters.search.as_deref().unwrap_or("");
                format!("/lists/{}/members", list_id)
            }
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Mailchimp object: {obj}"
                )));
            }
        };
        let resp = client
            .get(format!("{}{}", self.base_url(), path))
            .header("Authorization", self.auth_header())
            .query(&[("count", count.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Mailchimp list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[obj].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "campaigns" => "/campaigns".to_string(),
            "lists" => "/lists".to_string(),
            "members" => {
                let list_id = data["list_id"].as_str().unwrap_or("");
                format!("/lists/{}/members", list_id)
            }
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Mailchimp object: {obj}"
                )));
            }
        };
        let resp = client
            .post(format!("{}{}", self.base_url(), path))
            .header("Authorization", self.auth_header())
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Mailchimp create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "campaigns" => format!("/campaigns/{}", id),
            "lists" => format!("/lists/{}", id),
            "members" => {
                let list_id = data["list_id"].as_str().unwrap_or("");
                format!("/lists/{}/members/{}", list_id, id)
            }
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Mailchimp object: {obj}"
                )));
            }
        };
        let resp = client
            .patch(format!("{}{}", self.base_url(), path))
            .header("Authorization", self.auth_header())
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Mailchimp update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let path = match obj {
            "campaigns" => format!("/campaigns/{}", id),
            "lists" => format!("/lists/{}", id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Mailchimp object: {obj}"
                )));
            }
        };
        let resp = client
            .delete(format!("{}{}", self.base_url(), path))
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Mailchimp delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Mailchimp delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "send_campaign" => {
                let id = params["campaign_id"].as_str().unwrap_or("");
                format!("/campaigns/{}/actions/send", id)
            }
            "schedule_campaign" => {
                let id = params["campaign_id"].as_str().unwrap_or("");
                format!("/campaigns/{}/actions/schedule", id)
            }
            "add_member_to_list" => {
                let list_id = params["list_id"].as_str().unwrap_or("");
                format!("/lists/{}/members", list_id)
            }
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Mailchimp action: {action}"
                )));
            }
        };
        let resp = client
            .post(format!("{}{}", self.base_url(), path))
            .header("Authorization", self.auth_header())
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Mailchimp action failed: {e}")))?;
        // Mailchimp returns 204 for action endpoints that have no response body.
        if resp.status().as_u16() == 204 {
            Ok(serde_json::json!({ "status": "success" }))
        } else {
            crate::connectors::common::parse_json(resp).await
        }
    }
}
