//! HubSpot CRM connector for Clawz Gateway.
//!
//! Talks to the HubSpot v3 CRM API (OAuth2 or private-app token) and supports
//! standard CRM objects: contacts, companies, deals, tickets, etc.
//!
//! # Authentication modes
//! - OAuth2 — interactive user consent.
//! - Private app token (API key) — server-to-server with scoped permissions.
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

/// HubSpot CRM connector.
///
/// Internally stores either an active OAuth2 flow or a pre-shared private-app token.
/// The [`ApiClient`] helper is used to inject the correct `Authorization` header.
pub struct HubSpotConnector {
    /// OAuth2 flow configuration (populated when using the web-flow constructor).
    oauth: OAuth2Flow,
    /// Credentials after OAuth exchange, or a manually injected private-app token.
    credentials: Option<Credentials>,
}

impl HubSpotConnector {
    /// Create a new HubSpot connector using OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://app.hubspot.com/oauth/authorize",
            "https://api.hubapi.com/oauth/v1/token",
            vec![
                "oauth".into(),
                "crm.objects.contacts.read".into(),
                "crm.objects.contacts.write".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
        }
    }

    /// Create a new HubSpot connector with a private app token.
    ///
    /// Private apps are the recommended way for server-to-server integrations because
    /// they do not require refreshing access tokens.
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        let oauth = OAuth2Flow::new(
            "",
            "",
            "",
            "https://app.hubspot.com/oauth/authorize",
            "https://api.hubapi.com/oauth/v1/token",
            vec!["oauth".into()],
        );
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(api_key.into()),
                ..Default::default()
            }),
        }
    }

    /// Build an [`ApiClient`] targeting `https://api.hubapi.com`.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new("https://api.hubapi.com", creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for HubSpotConnector {
    fn platform_id(&self) -> &str {
        "hubspot"
    }

    fn display_name(&self) -> &str {
        "HubSpot"
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
        let resp = client
            .get(&format!("/crm/v3/objects/{obj}"))
            .query(&[("limit", limit.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("HubSpot list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let resp = client
            .post(&format!("/crm/v3/objects/{obj}"))
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("HubSpot create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let resp = client
            .patch(&format!("/crm/v3/objects/{obj}/{id}"))
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("HubSpot update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let resp = client
            .delete(&format!("/crm/v3/objects/{obj}/{id}"))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("HubSpot delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "HubSpot delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "engagement" => "/engagements/v1/engagements".to_string(),
            "workflow" => format!(
                "/automation/v4/workflows/{}/enrollments",
                params
                    .get("workflowId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
            _ => format!("/crm/v3/objects/{action}"),
        };
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("HubSpot action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
