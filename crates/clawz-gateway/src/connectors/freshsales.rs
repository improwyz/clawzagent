//! # Freshsales Connector
//!
//! Integrates with the Freshsales (Freshworks CRM) API. Supports contacts, deals,
//! leads, accounts, and tasks. Authentication is via a single API key sent in the
//! `Authorization` header as a token.
//!
//! ## Supported Objects
//! - `contacts`, `deals`, `leads`, `accounts`, `tasks`
//!
//! ## Supported Actions
//! - `search`, `bulk_destroy`
//!
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.
//! // Dependency: `reqwest::Client` used directly to attach the custom token header.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Freshsales / Freshworks CRM connector.
///
/// Stores an API key and a Freshworks subdomain. The subdomain is used to construct
/// the per-tenant base URL (`https://<domain>.myfreshworks.com/crm/sales/api`).
pub struct FreshsalesConnector {
    /// Freshsales API key.
    api_key: String,
    /// Freshworks subdomain, e.g. `mycompany`.
    domain: String,
    /// Reusable HTTP client.
    client: Client,
}

impl FreshsalesConnector {
    /// Create a new Freshsales connector.
    /// `domain` is the Freshworks subdomain, e.g. "mycompany".
    pub fn new(api_key: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            domain: domain.into(),
            client: Client::new(),
        }
    }

    /// Build the per-tenant base URL from the configured subdomain.
    fn base_url(&self) -> String {
        format!("https://{}.myfreshworks.com/crm/sales/api", self.domain)
    }
}

#[async_trait]
impl SaaSConnector for FreshsalesConnector {
    fn platform_id(&self) -> &str {
        "freshsales"
    }

    fn display_name(&self) -> &str {
        "Freshsales"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth("Freshsales uses API key authentication".into()))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth("Freshsales uses API key authentication".into()))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        let path = match obj {
            "contacts" => "/contacts",
            "deals" => "/deals",
            "leads" => "/leads",
            "accounts" => "/sales_accounts",
            "tasks" => "/tasks",
            _ => return Err(ClawzError::Provider(format!("Unknown Freshsales object: {obj}"))),
        };
        let per_page = filters.limit.unwrap_or(100).min(100);
        let resp = self.client
            .get(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .query(&[("per_page", per_page.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Freshsales list failed: {e}")))?;
        // Freshsales uses "sales_accounts" as the JSON key for accounts, so we normalize it.
        let key = match obj {
            "accounts" => "sales_accounts",
            other => other,
        };
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[key].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let (path, key) = match obj {
            "contacts" => ("/contacts", "contact"),
            "deals" => ("/deals", "deal"),
            "leads" => ("/leads", "lead"),
            "accounts" => ("/sales_accounts", "sales_account"),
            _ => return Err(ClawzError::Provider(format!("Unknown Freshsales object: {obj}"))),
        };
        // Freshsales expects the object wrapped under its singular key.
        let body = serde_json::json!({ key: data });
        let resp = self.client
            .post(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Freshsales create failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[key].clone())
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let (path, key) = match obj {
            "contacts" => (format!("/contacts/{}", id), "contact"),
            "deals" => (format!("/deals/{}", id), "deal"),
            "leads" => (format!("/leads/{}", id), "lead"),
            "accounts" => (format!("/sales_accounts/{}", id), "sales_account"),
            _ => return Err(ClawzError::Provider(format!("Unknown Freshsales object: {obj}"))),
        };
        let body = serde_json::json!({ key: data });
        let resp = self.client
            .put(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Freshsales update failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[key].clone())
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let path = match obj {
            "contacts" => format!("/contacts/{}", id),
            "deals" => format!("/deals/{}", id),
            "leads" => format!("/leads/{}", id),
            "accounts" => format!("/sales_accounts/{}", id),
            _ => return Err(ClawzError::Provider(format!("Unknown Freshsales object: {obj}"))),
        };
        let resp = self.client
            .delete(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Freshsales delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Freshsales delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let path = match action {
            "search" => {
                let q = params["q"].as_str().unwrap_or("");
                format!("/search?q={}", q)
            }
            "bulk_destroy" => format!("/{}/bulk_destroy", params["type"].as_str().unwrap_or("contacts")),
            _ => format!("/{}", action),
        };
        let resp = self.client
            .post(format!("{}{}", self.base_url(), path))
            .header("Authorization", format!("Token token={}", self.api_key))
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Freshsales action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
