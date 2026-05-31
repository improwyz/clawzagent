//! Salesforce CRM connector for Clawz Gateway.
//!
//! Integrates with the Salesforce REST API (v59.0). Supports standard and custom
//! objects via SOQL queries, SObject CRUD and Apex REST actions.
//!
//! # Authentication
//! - OAuth2 (web flow) — stores the `instance_url` returned in the token response.
//! - API key + explicit `instance_url` — mainly for testing / development.
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

/// Salesforce CRM connector.
///
/// Salesforce is unique among the connectors because the API base URL is dynamic:
/// it is returned as `instance_url` during the OAuth2 exchange. The connector
/// therefore stores `instance_url` separately and falls back to the login domain
/// when an API-key constructor is used.
pub struct SalesforceConnector {
    /// OAuth2 flow configuration.
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange (or a manually injected API key).
    credentials: Option<Credentials>,
    /// Salesforce instance base URL (e.g. `https://na1.salesforce.com`).
    instance_url: Option<String>,
}

impl SalesforceConnector {
    /// Create a new Salesforce connector using OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://login.salesforce.com/services/oauth2/authorize",
            "https://login.salesforce.com/services/oauth2/token",
            vec!["api".into(), "refresh_token".into()],
        );
        Self {
            oauth,
            credentials: None,
            instance_url: None,
        }
    }

    /// Create a new Salesforce connector with API key (for testing).
    ///
    /// The `instance_url` must be provided because it cannot be discovered via
    /// OAuth2 token response in this mode.
    pub fn with_api_key(api_key: impl Into<String>, instance_url: impl Into<String>) -> Self {
        let oauth = OAuth2Flow::new(
            "",
            "",
            "",
            "https://login.salesforce.com/services/oauth2/authorize",
            "https://login.salesforce.com/services/oauth2/token",
            vec!["api".into(), "refresh_token".into()],
        );
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(api_key.into()),
                ..Default::default()
            }),
            instance_url: Some(instance_url.into()),
        }
    }

    /// Build an [`ApiClient`] against the resolved instance URL.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        let base = self
            .instance_url
            .as_ref()
            .cloned()
            .unwrap_or_else(|| "https://login.salesforce.com".into());
        Ok(ApiClient::new(base, creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for SalesforceConnector {
    fn platform_id(&self) -> &str {
        "salesforce"
    }

    fn display_name(&self) -> &str {
        "Salesforce"
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
        // Use FIELDS(STANDARD) so the query works for both standard and custom objects
        // without requiring an explicit field list.
        let query = format!("SELECT FIELDS(STANDARD) FROM {obj} LIMIT {limit}");
        let resp = client
            .get("/services/data/v59.0/query")
            .query(&[("q", query)])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Salesforce list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json
            .get("records")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let resp = client
            .post(&format!("/services/data/v59.0/sobjects/{obj}/"))
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Salesforce create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let resp = client
            .patch(&format!("/services/data/v59.0/sobjects/{obj}/{id}"))
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Salesforce update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let resp = client
            .delete(&format!("/services/data/v59.0/sobjects/{obj}/{id}"))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Salesforce delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Salesforce delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "query" => "/services/data/v59.0/query".to_string(),
            "search" => "/services/data/v59.0/search".to_string(),
            // Any other action is assumed to be a custom Apex REST class.
            _ => format!("/services/apexrest/{action}"),
        };
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Salesforce action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
