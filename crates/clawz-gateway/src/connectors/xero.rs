//! Xero connector for Clawz Gateway.
//!
//! Integrates with the Xero Accounting API (v2.0) and the Xero Identity API.
//! Supports invoices, contacts, accounts, payments and bank transactions.
//!
//! # Authentication
//! OAuth2 against Xero Identity. After exchange, the `tenant_id` (organisation)
//! must be supplied on every request via the `Xero-Tenant-Id` header.
//!
//! # Important Xero-isms
//! - `delete_object` for invoices actually **voids** them (sets status to VOIDED)
//!   because Xero does not support hard-deletion of posted invoices.
//! - `get_tenants` is exposed as an action so callers can discover available
//!   organisations after OAuth2.
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

/// Xero connector.
///
/// Every Accounting API call must include the `Xero-Tenant-Id` header. The connector
/// stores the tenant ID after OAuth exchange (or it can be injected manually).
pub struct XeroConnector {
    /// OAuth2 flow configuration for Xero Identity.
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange.
    credentials: Option<Credentials>,
    /// Target Xero organisation GUID (required on every request).
    tenant_id: Option<String>,
}

impl XeroConnector {
    /// Create a new Xero connector.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://login.xero.com/identity/connect/authorize",
            "https://identity.xero.com/connect/token",
            vec![
                "openid".into(),
                "profile".into(),
                "email".into(),
                "accounting.transactions".into(),
                "accounting.contacts".into(),
                "accounting.settings".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
            tenant_id: None,
        }
    }

    /// Build an [`ApiClient`] against the Xero Accounting API root.
    fn client(&self) -> Result<ApiClient> {
        let creds = self
            .credentials
            .as_ref()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))?;
        Ok(ApiClient::new(
            "https://api.xero.com/api.xro/2.0",
            creds.clone(),
        ))
    }

    /// Resolve the tenant ID for the `Xero-Tenant-Id` header.
    fn tenant_header(&self) -> String {
        self.tenant_id.clone().unwrap_or_default()
    }
}

#[async_trait]
impl SaaSConnector for XeroConnector {
    fn platform_id(&self) -> &str {
        "xero"
    }

    fn display_name(&self) -> &str {
        "Xero"
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

    async fn list_objects(&self, obj: &str, _filters: &Filters) -> Result<Vec<Value>> {
        let client = self.client()?;
        let path = match obj {
            "invoices" => "/Invoices",
            "contacts" => "/Contacts",
            "accounts" => "/Accounts",
            "payments" => "/Payments",
            "bank_transactions" => "/BankTransactions",
            _ => return Err(ClawzError::Provider(format!("Unknown Xero object: {obj}"))),
        };
        let resp = client
            .get(path)
            .header("Xero-Tenant-Id", self.tenant_header())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Xero list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        let key = match obj {
            "invoices" => "Invoices",
            "contacts" => "Contacts",
            "accounts" => "Accounts",
            "payments" => "Payments",
            "bank_transactions" => "BankTransactions",
            _ => obj,
        };
        Ok(json[key].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "invoices" => "/Invoices",
            "contacts" => "/Contacts",
            "payments" => "/Payments",
            _ => return Err(ClawzError::Provider(format!("Unknown Xero object: {obj}"))),
        };
        let resp = client
            .post(path)
            .header("Xero-Tenant-Id", self.tenant_header())
            .header("Accept", "application/json")
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Xero create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let client = self.client()?;
        let path = match obj {
            "invoices" => format!("/Invoices/{}", id),
            "contacts" => format!("/Contacts/{}", id),
            _ => return Err(ClawzError::Provider(format!("Unknown Xero object: {obj}"))),
        };
        let resp = client
            .post(&path)
            .header("Xero-Tenant-Id", self.tenant_header())
            .header("Accept", "application/json")
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Xero update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let client = self.client()?;
        let path = match obj {
            "invoices" => format!("/Invoices/{}", id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Cannot delete Xero {obj} directly; void instead"
                )));
            }
        };
        // Xero doesn't delete invoices, it voids them via VOIDED status
        let body = serde_json::json!({ "Status": "VOIDED" });
        let resp = client
            .post(&path)
            .header("Xero-Tenant-Id", self.tenant_header())
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Xero void failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Xero void failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let client = self.client()?;
        match action {
            "get_tenants" => {
                // The tenants (organisations) endpoint lives on the Xero Identity domain
                // and requires a raw bearer token rather than the ApiClient abstraction.
                let resp = reqwest::Client::new()
                    .get("https://api.xero.com/connections")
                    .bearer_auth(
                        self.credentials
                            .as_ref()
                            .and_then(|c| c.access_token.as_deref())
                            .unwrap_or(""),
                    )
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Xero get tenants failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "email_invoice" => {
                let id = params["invoice_id"].as_str().unwrap_or("");
                let resp = client
                    .post(&format!("/Invoices/{}/Email", id))
                    .header("Xero-Tenant-Id", self.tenant_header())
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Xero email invoice failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Xero action: {action}"
            ))),
        }
    }
}
