//! QuickBooks Online connector for Clawz Gateway.
//!
//! Integrates with the Intuit QuickBooks Online Accounting API (v3). Supports
//! invoices, customers, payments, accounts, bills and vendors. Uses OAuth2 for
//! user-facing flows and can toggle between production and sandbox environments.
//!
//! # Key behaviour
//! - `list_objects` translates the generic object name into a QBO SQL-like query
//!   (`SELECT * FROM <Entity> MAXRESULTS N`).
//! - `update_object` uses `operation=update` with `minorversion=65`.
//! - `delete_object` uses `operation=delete` and requires `SyncToken`.
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

/// QuickBooks Online connector.
///
/// The connector needs a `realm_id` (company file ID) to build the correct base URL:
/// `https://{sandbox-}quickbooks.api.intuit.com/v3/company/{realm_id}`.
pub struct QuickBooksConnector {
    /// OAuth2 flow configuration for Intuit’s auth servers.
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange.
    credentials: Option<Credentials>,
    /// QuickBooks company file / realm ID.
    realm_id: Option<String>,
    /// When `true`, targets the sandbox environment (`sandbox-quickbooks.api.intuit.com`).
    sandbox: bool,
}

impl QuickBooksConnector {
    /// Create a new QuickBooks connector.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        sandbox: bool,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://appcenter.intuit.com/connect/oauth2",
            "https://oauth.platform.intuit.com/oauth2/v1/tokens/bearer",
            vec!["com.intuit.quickbooks.accounting".into()],
        );
        Self { oauth, credentials: None, realm_id: None, sandbox }
    }

    /// Build the per-realm base URL.
    fn base_url(&self) -> String {
        let env = if self.sandbox { "sandbox-quickbooks" } else { "quickbooks" };
        let realm = self.realm_id.as_deref().unwrap_or("");
        format!("https://{}.api.intuit.com/v3/company/{}", env, realm)
    }

    /// Build an [`ApiClient`] targeting the realm-specific endpoint.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new(self.base_url(), creds.clone()))
    }
}

#[async_trait]
impl SaaSConnector for QuickBooksConnector {
    fn platform_id(&self) -> &str {
        "quickbooks"
    }

    fn display_name(&self) -> &str {
        "QuickBooks"
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
        let max_results = filters.limit.unwrap_or(100).min(1000);
        let entity = match obj {
            "invoices" => "Invoice",
            "customers" => "Customer",
            "payments" => "Payment",
            "accounts" => "Account",
            "bills" => "Bill",
            "vendors" => "Vendor",
            _ => return Err(ClawzError::Provider(format!("Unknown QuickBooks object: {obj}"))),
        };
        let query = format!("SELECT * FROM {} MAXRESULTS {}", entity, max_results);
        let resp = client
            .get("/query")
            .query(&[("query", query), ("minorversion", "65".into())])
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("QuickBooks list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        let entities = json["QueryResponse"][entity]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(entities)
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let entity = match obj {
            "invoices" => "invoice",
            "customers" => "customer",
            "payments" => "payment",
            "bills" => "bill",
            "vendors" => "vendor",
            _ => return Err(ClawzError::Provider(format!("Unknown QuickBooks object: {obj}"))),
        };
        let resp = client
            .post(&format!("/{}", entity))
            .query(&[("minorversion", "65")])
            .header("Accept", "application/json")
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("QuickBooks create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let entity = match obj {
            "invoices" => "invoice",
            "customers" => "customer",
            "payments" => "payment",
            _ => return Err(ClawzError::Provider(format!("Unknown QuickBooks object: {obj}"))),
        };
        // QuickBooks requires sparse=true for partial updates; the minorversion and
        // operation query params signal an update rather than a create.
        let resp = client
            .post(&format!("/{}", entity))
            .query(&[("minorversion", "65"), ("operation", "update")])
            .header("Accept", "application/json")
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("QuickBooks update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let entity = match obj {
            "invoices" => "invoice",
            "bills" => "bill",
            _ => return Err(ClawzError::Provider(format!("Cannot delete QuickBooks {obj}"))),
        };
        // QBO delete requires the Id and a SyncToken (0 is acceptable for deletes).
        let body = serde_json::json!({ "Id": id, "SyncToken": "0" });
        let resp = client
            .post(&format!("/{}", entity))
            .query(&[("minorversion", "65"), ("operation", "delete")])
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("QuickBooks delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "QuickBooks delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        match action {
            "send_invoice" => {
                let id = params["id"].as_str().unwrap_or("");
                let email = params["email"].as_str().unwrap_or("");
                let resp = client
                    .post(&format!("/invoice/{}/send", id))
                    .query(&[("sendTo", email), ("minorversion", "65")])
                    .header("Accept", "application/json")
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("QuickBooks send invoice failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "void_invoice" => {
                let body = serde_json::json!({ "Id": params["id"], "SyncToken": "0" });
                let resp = client
                    .post("/invoice")
                    .query(&[("minorversion", "65"), ("operation", "void")])
                    .header("Accept", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("QuickBooks void invoice failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown QuickBooks action: {action}"))),
        }
    }
}
