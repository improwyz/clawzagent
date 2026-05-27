//! # Datadog Connector
//!
//! Integrates with the Datadog REST API (v1 and v2). Supports monitors, dashboards,
//! hosts, metrics, events, and logs. Authentication uses a two-key system: an API key
//! and an Application key, both sent as headers.
//!
//! ## Supported Objects
//! - `monitors`, `dashboards`, `hosts`, `metrics`, `events`, `logs`
//!
//! ## Supported Actions
//! - `mute_monitor`, `query_metrics`, `submit_metrics`
//!
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.
//! // Dependency: `reqwest::Client` used directly because every request needs two custom headers.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Datadog connector.
///
/// Stores an API key, an Application key, and the Datadog site domain. The site
/// parameter allows switching between US (`datadoghq.com`) and EU (`datadoghq.eu`)
/// regions without changing code.
pub struct DatadogConnector {
    /// Datadog API key.
    api_key: String,
    /// Datadog Application key.
    app_key: String,
    /// Datadog site domain, e.g. `datadoghq.com`.
    site: String,
    /// Reusable HTTP client.
    client: Client,
}

impl DatadogConnector {
    /// Create a new Datadog connector.
    /// `site` is the Datadog site, e.g. "datadoghq.com" or "datadoghq.eu".
    pub fn new(
        api_key: impl Into<String>,
        app_key: impl Into<String>,
        site: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            app_key: app_key.into(),
            site: site.into(),
            client: Client::new(),
        }
    }

    /// Build the full API base URL from the configured site.
    fn base_url(&self) -> String {
        format!("https://api.{}", self.site)
    }

    /// Build an authenticated GET request with both Datadog headers.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.base_url(), path))
            .header("DD-API-KEY", &self.api_key)
            .header("DD-APPLICATION-KEY", &self.app_key)
    }

    /// Build an authenticated POST request with both Datadog headers.
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{}", self.base_url(), path))
            .header("DD-API-KEY", &self.api_key)
            .header("DD-APPLICATION-KEY", &self.app_key)
    }

    /// Build an authenticated PUT request with both Datadog headers.
    fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .put(format!("{}{}", self.base_url(), path))
            .header("DD-API-KEY", &self.api_key)
            .header("DD-APPLICATION-KEY", &self.app_key)
    }

    /// Build an authenticated DELETE request with both Datadog headers.
    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}{}", self.base_url(), path))
            .header("DD-API-KEY", &self.api_key)
            .header("DD-APPLICATION-KEY", &self.app_key)
    }
}

#[async_trait]
impl SaaSConnector for DatadogConnector {
    fn platform_id(&self) -> &str {
        "datadog"
    }

    fn display_name(&self) -> &str {
        "Datadog"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth(
            "Datadog uses API + Application key authentication".into(),
        ))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth(
            "Datadog uses API + Application key authentication".into(),
        ))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let limit = filters.limit.unwrap_or(100).min(1000);
        let path = match obj {
            "monitors" => format!("/api/v1/monitor?count={}", limit),
            "dashboards" => "/api/v1/dashboard".to_string(),
            "hosts" => format!("/api/v1/hosts?count={}", limit),
            "metrics" => {
                let from = filters.search.as_deref().unwrap_or("now-1h");
                format!("/api/v1/metrics?from={}", from)
            }
            "events" => format!("/api/v1/events?count={}", limit),
            "logs" => "/api/v2/logs/events".to_string(),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Datadog object: {obj}"
                )));
            }
        };
        let resp = self
            .get(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Datadog list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Datadog uses different response keys per endpoint; normalize them here.
        let data = match obj {
            "monitors" => json.as_array().cloned().unwrap_or_default(),
            "dashboards" => json["dashboards"].as_array().cloned().unwrap_or_default(),
            "hosts" => json["host_list"].as_array().cloned().unwrap_or_default(),
            "metrics" => json["metrics"].as_array().cloned().unwrap_or_default(),
            "events" => json["events"].as_array().cloned().unwrap_or_default(),
            "logs" => json["data"].as_array().cloned().unwrap_or_default(),
            _ => vec![],
        };
        Ok(data)
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "monitors" => "/api/v1/monitor",
            "dashboards" => "/api/v1/dashboard",
            "events" => "/api/v1/events",
            "downtimes" => "/api/v1/downtime",
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Datadog object: {obj}"
                )));
            }
        };
        let resp = self
            .post(path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Datadog create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let path = match obj {
            "monitors" => format!("/api/v1/monitor/{}", id),
            "dashboards" => format!("/api/v1/dashboard/{}", id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Datadog object: {obj}"
                )));
            }
        };
        let resp = self
            .put(&path)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Datadog update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "monitors" => format!("/api/v1/monitor/{}", id),
            "dashboards" => format!("/api/v1/dashboard/{}", id),
            "downtimes" => format!("/api/v1/downtime/{}", id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Datadog object: {obj}"
                )));
            }
        };
        let resp = self
            .delete(&path)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Datadog delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Datadog delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        match action {
            "mute_monitor" => {
                let id = params["id"].as_str().unwrap_or("");
                let resp = self
                    .post(&format!("/api/v1/monitor/{}/mute", id))
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("Datadog mute monitor failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            "query_metrics" => {
                let from = params["from"].as_i64().unwrap_or(0);
                let to = params["to"].as_i64().unwrap_or(0);
                let query = params["query"].as_str().unwrap_or("");
                let resp = self
                    .get(&format!(
                        "/api/v1/query?from={}&to={}&query={}",
                        from, to, query
                    ))
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("Datadog query metrics failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            "submit_metrics" => {
                let resp = self
                    .post("/api/v2/series")
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("Datadog submit metrics failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Datadog action: {action}"
            ))),
        }
    }
}
