//! Zoho CRM connector for Clawz Gateway.
//!
//! Integrates with the Zoho CRM REST API v2. Supports contacts, deals, leads,
//! accounts and tasks. Automatically routes to the correct regional data centre
//! based on the `region` parameter.
//!
//! # Authentication
//! OAuth2 against Zoho Accounts. The `region` parameter (`us`, `eu`, `au`, `in`)
//! determines both the OAuth and API base URLs.
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

/// Zoho CRM connector.
///
/// Zoho shards customers across regional domains (`.com`, `.eu`, `.com.au`, `.in`).
/// Both the OAuth endpoints and the CRM API base URL are derived from the `region`
/// field so requests always hit the correct data centre.
pub struct ZohoConnector {
    /// OAuth2 flow configuration scoped to the regional auth domain.
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange.
    credentials: Option<Credentials>,
    /// Regional data-centre code: `us`, `eu`, `au` or `in`.
    region: String,
}

impl ZohoConnector {
    /// Create a new Zoho CRM connector.
    ///
    /// # Arguments
    /// - `region` — One of `us`, `eu`, `au`, `in`. Defaults to `us` if unknown.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        let region = region.into();
        let auth_domain = match region.as_str() {
            "eu" => "https://accounts.zoho.eu",
            "au" => "https://accounts.zoho.com.au",
            "in" => "https://accounts.zoho.in",
            _ => "https://accounts.zoho.com",
        };
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            format!("{}/oauth/v2/auth", auth_domain),
            format!("{}/oauth/v2/token", auth_domain),
            vec!["ZohoCRM.modules.ALL".into(), "ZohoCRM.settings.ALL".into()],
        );
        Self {
            oauth,
            credentials: None,
            region,
        }
    }

    /// Resolve the regional CRM API base URL from the `region` field.
    fn base_url(&self) -> String {
        match self.region.as_str() {
            "eu" => "https://www.zohoapis.eu/crm/v2".into(),
            "au" => "https://www.zohoapis.com.au/crm/v2".into(),
            "in" => "https://www.zohoapis.in/crm/v2".into(),
            _ => "https://www.zohoapis.com/crm/v2".into(),
        }
    }

    /// Build an [`ApiClient`] against the regional CRM endpoint.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new(self.base_url(), creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for ZohoConnector {
    fn platform_id(&self) -> &str {
        "zoho"
    }

    fn display_name(&self) -> &str {
        "Zoho CRM"
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
        // Map generic object names to Zoho CRM module names.
        let module = match obj {
            "contacts" => "Contacts",
            "deals" => "Deals",
            "leads" => "Leads",
            "accounts" => "Accounts",
            "tasks" => "Tasks",
            _ => obj,
        };
        // Zoho CRM caps per_page at 200.
        let per_page = filters.limit.unwrap_or(200).min(200);
        let resp = client
            .get(&format!("/{}", module))
            .query(&[("per_page", per_page.to_string())])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Zoho list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let module = match obj {
            "contacts" => "Contacts",
            "deals" => "Deals",
            "leads" => "Leads",
            "accounts" => "Accounts",
            _ => obj,
        };
        // Zoho CRM expects a wrapper `{ "data": [ { ... } ] }`.
        let body = serde_json::json!({ "data": [data] });
        let resp = client
            .post(&format!("/{}", module))
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Zoho create failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"][0].clone())
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let module = match obj {
            "contacts" => "Contacts",
            "deals" => "Deals",
            "leads" => "Leads",
            "accounts" => "Accounts",
            _ => obj,
        };
        let body = serde_json::json!({ "data": [data] });
        let resp = client
            .put(&format!("/{}/{}", module, id))
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Zoho update failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"][0].clone())
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let module = match obj {
            "contacts" => "Contacts",
            "deals" => "Deals",
            "leads" => "Leads",
            "accounts" => "Accounts",
            _ => obj,
        };
        let resp = client
            .delete(&format!("/{}/{}", module, id))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Zoho delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Zoho delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match action {
            "search" => {
                let module = params["module"].as_str().unwrap_or("Contacts");
                let criteria = params["criteria"].as_str().unwrap_or("");
                format!("/{}/search?criteria={}", module, criteria)
            }
            "convert_lead" => {
                let id = params["id"].as_str().unwrap_or("");
                format!("/Leads/{}/actions/convert", id)
            }
            _ => format!(
                "/{}/?action={}",
                params["module"].as_str().unwrap_or("Contacts"),
                action
            ),
        };
        let resp = client
            .post(&path)
            .json(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Zoho action failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }
}
