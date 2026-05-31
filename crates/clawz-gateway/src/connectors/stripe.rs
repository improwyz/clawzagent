//! Stripe payments connector for Clawz Gateway.
//!
//! Integrates with the Stripe REST API. Supports charges, customers, invoices,
//! subscriptions, refunds and payouts.
//!
//! # Authentication
//! API key only (secret key). Stripe OAuth2 (Connect) is not implemented in this
//! connector.
//!
//! # Cross-module dependencies
//! - [`ApiClient`] from `crate::connectors::common`.
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

// Dependency: common::ApiClient
use crate::connectors::common::ApiClient;
// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Stripe payments connector.
///
/// All requests target `https://api.stripe.com`. The connector uses [`ApiClient`]
/// to inject the `Authorization: Bearer <secret_key>` header.
pub struct StripeConnector {
    /// Credentials holding the Stripe secret API key.
    credentials: Option<Credentials>,
}

impl StripeConnector {
    /// Create a new Stripe connector with a secret API key.
    pub fn new(secret_key: impl Into<String>) -> Self {
        Self {
            credentials: Some(Credentials {
                api_key: Some(secret_key.into()),
                ..Default::default()
            }),
        }
    }

    /// Build an [`ApiClient`] against the Stripe API root.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new("https://api.stripe.com", creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for StripeConnector {
    fn platform_id(&self) -> &str {
        "stripe"
    }

    fn display_name(&self) -> &str {
        "Stripe"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth(
            "Stripe uses API key authentication, not OAuth2".into(),
        ))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth(
            "Stripe uses API key authentication, not OAuth2".into(),
        ))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let client = self.client()?;
        let limit = filters.limit.unwrap_or(10);
        let resp = client
            .get(&format!("/v1/{obj}"))
            .query(&[("limit", limit.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Stripe list failed: {e}")))?;

        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let resp = client
            .post(&format!("/v1/{obj}"))
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Stripe create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let resp = client
            .post(&format!("/v1/{obj}/{id}"))
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Stripe update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let resp = client
            .delete(&format!("/v1/{obj}/{id}"))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Stripe delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Stripe delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "charge" => "/v1/charges".to_string(),
            "refund" => "/v1/refunds".to_string(),
            "invoice" => "/v1/invoices".to_string(),
            "subscription" => "/v1/subscriptions".to_string(),
            "payout" => "/v1/payouts".to_string(),
            _ => format!("/v1/{action}"),
        };
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Stripe action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
