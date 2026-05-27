//! Google Analytics 4 connector for Clawz Gateway.
//!
//! This module implements a read-only (plus custom report execution) connector for
//! Google Analytics 4 (GA4). It uses Google’s OAuth2 flow and targets the
//! Analytics Admin and Analytics Data REST APIs.
//!
//! # Key behaviour
//! - `list_objects` for `"properties"` enumerates GA4 properties the user owns.
//! - `list_objects` for `"reports"`, `"metrics"` or `"dimensions"` runs a canned
//!   report for the last 30 days (or a user-supplied date range).
//! - Create / update / delete are explicitly unsupported because GA4 is
//!   fundamentally an analytics-read platform.
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

/// Google Analytics 4 connector.
///
/// Stores the OAuth2 flow state, optional credentials, and an optional default GA4
/// property ID so callers don’t have to pass it on every request.
pub struct GoogleAnalyticsConnector {
    /// OAuth2 flow configuration for Google’s auth servers.
    oauth: OAuth2Flow,
    /// Credentials populated after a successful OAuth exchange.
    credentials: Option<Credentials>,
    /// Default GA4 property ID (format: `properties/123456789`).
    property_id: Option<String>,
}

impl GoogleAnalyticsConnector {
    /// Create a new Google Analytics connector.
    ///
    /// # Arguments
    /// - `property_id` — Optional default GA4 property. Can be overridden per-request.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        property_id: Option<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            vec![
                "https://www.googleapis.com/auth/analytics.readonly".into(),
                "https://www.googleapis.com/auth/analytics".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
            property_id,
        }
    }

    /// Extract the current OAuth access token from stored credentials.
    fn token(&self) -> Result<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.clone())
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))
    }

    /// Resolve the property ID to use for a report request.
    /// Falls back to the default set at construction time.
    fn property(&self) -> Result<String> {
        self.property_id
            .clone()
            .ok_or_else(|| ClawzError::Provider("property_id required for GA4".into()))
    }
}

#[async_trait]
impl SaaSConnector for GoogleAnalyticsConnector {
    fn platform_id(&self) -> &str {
        "google_analytics"
    }

    fn display_name(&self) -> &str {
        "Google Analytics"
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
        let token = self.token()?;
        match obj {
            "properties" => {
                let resp = reqwest::Client::new()
                    .get("https://analyticsadmin.googleapis.com/v1alpha/properties")
                    .bearer_auth(&token)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("GA list properties failed: {e}")))?;
                let json: Value = crate::connectors::common::parse_json(resp).await?;
                Ok(json["properties"].as_array().cloned().unwrap_or_default())
            }
            "reports" | "metrics" | "dimensions" => {
                // Run a basic report for the last 30 days unless the caller overrides the range
                // via `filters.search` (expected format "startDate:endDate").
                let property = self.property()?;
                let date_range = filters.search.as_deref().unwrap_or("30daysAgo:today");
                let parts: Vec<&str> = date_range.split(':').collect();
                let (start, end) = (
                    parts.first().copied().unwrap_or("30daysAgo"),
                    parts.last().copied().unwrap_or("today"),
                );
                let body = serde_json::json!({
                    "dateRanges": [{ "startDate": start, "endDate": end }],
                    "dimensions": [{ "name": "date" }],
                    "metrics": [
                        { "name": "sessions" },
                        { "name": "users" },
                        { "name": "pageviews" }
                    ]
                });
                let resp = reqwest::Client::new()
                    .post(format!(
                        "https://analyticsdata.googleapis.com/v1beta/{}:runReport",
                        property
                    ))
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("GA run report failed: {e}")))?;
                let json: Value = crate::connectors::common::parse_json(resp).await?;
                Ok(json["rows"].as_array().cloned().unwrap_or_default())
            }
            _ => Err(ClawzError::Provider(format!("Unknown GA object: {obj}"))),
        }
    }

    /// GA4 is a read-only analytics system; creation is not supported.
    async fn create_object(&self, obj: &str, _data: Value) -> Result<Value> {
        Err(ClawzError::Provider(format!(
            "Google Analytics does not support creating {obj}"
        )))
    }

    /// GA4 is a read-only analytics system; updates are not supported.
    async fn update_object(&self, obj: &str, _id: &str, _data: Value) -> Result<Value> {
        Err(ClawzError::Provider(format!(
            "Google Analytics does not support updating {obj}"
        )))
    }

    /// GA4 is a read-only analytics system; deletion is not supported.
    async fn delete_object(&self, obj: &str, _id: &str) -> Result<()> {
        Err(ClawzError::Provider(format!(
            "Google Analytics does not support deleting {obj}"
        )))
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let token = self.token()?;
        // Allow the caller to override the property ID per-action.
        let property = params["property_id"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| self.property_id.clone())
            .ok_or_else(|| ClawzError::Provider("property_id required".into()))?;

        match action {
            "run_report" => {
                let resp = reqwest::Client::new()
                    .post(format!(
                        "https://analyticsdata.googleapis.com/v1beta/{}:runReport",
                        property
                    ))
                    .bearer_auth(&token)
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("GA run report failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "run_realtime_report" => {
                let resp = reqwest::Client::new()
                    .post(format!(
                        "https://analyticsdata.googleapis.com/v1beta/{}:runRealtimeReport",
                        property
                    ))
                    .bearer_auth(&token)
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("GA realtime report failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown GA action: {action}"))),
        }
    }
}
