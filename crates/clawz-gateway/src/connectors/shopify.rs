//! Shopify connector for Clawz Gateway.
//!
//! Integrates with the Shopify Admin REST API (2024-01). Supports products,
//! orders, customers and collections. Uses OAuth2 for public apps and access
//! tokens for private apps.
//!
//! # Authentication
//! - OAuth2 (public app) — user installs the app and grants scopes.
//! - Access token (private app) — generated in the Shopify admin and injected
//!   directly.
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

/// Shopify connector.
///
/// The `shop` field is the myshopify subdomain (e.g. `mystore`). It is used to
/// build the per-store base URL: `https://{shop}.myshopify.com/admin/api/2024-01`.
pub struct ShopifyConnector {
    /// OAuth2 flow configuration (active for public-app integrations).
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange, or a manually injected token.
    credentials: Option<Credentials>,
    /// Shopify store subdomain (without `.myshopify.com`).
    shop: String,
}

impl ShopifyConnector {
    /// Create a new Shopify connector via OAuth2.
    ///
    /// `shop` is the myshopify.com subdomain, e.g. "mystore".
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        shop: impl Into<String>,
    ) -> Self {
        let shop = shop.into();
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            format!("https://{shop}.myshopify.com/admin/oauth/authorize"),
            format!("https://{shop}.myshopify.com/admin/oauth/access_token"),
            vec![
                "read_products".into(),
                "write_products".into(),
                "read_orders".into(),
                "write_orders".into(),
                "read_customers".into(),
                "write_customers".into(),
            ],
        );
        Self {
            oauth,
            credentials: None,
            shop,
        }
    }

    /// Create with an access token (for private apps).
    ///
    /// Private apps are created inside a single store and do not participate in
    /// the Shopify OAuth2 flow.
    pub fn with_token(token: impl Into<String>, shop: impl Into<String>) -> Self {
        let shop_str = shop.into();
        let oauth = OAuth2Flow::new("", "", "", "", "", vec![]);
        Self {
            oauth,
            credentials: Some(Credentials {
                access_token: Some(token.into()),
                ..Default::default()
            }),
            shop: shop_str,
        }
    }

    /// Build the per-store Admin API base URL.
    fn base_url(&self) -> String {
        format!("https://{}.myshopify.com/admin/api/2024-01", self.shop)
    }

    /// Extract the Shopify access token from stored credentials.
    fn token(&self) -> Result<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.clone())
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }
}

#[async_trait]
impl SaaSConnector for ShopifyConnector {
    fn platform_id(&self) -> &str {
        "shopify"
    }

    fn display_name(&self) -> &str {
        "Shopify"
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
        // Shopify caps most list endpoints at 250 records per page.
        let limit = filters.limit.unwrap_or(250).min(250);
        let path = match obj {
            "products" => format!("{}/products.json?limit={}", self.base_url(), limit),
            "orders" => format!("{}/orders.json?limit={}&status=any", self.base_url(), limit),
            "customers" => format!("{}/customers.json?limit={}", self.base_url(), limit),
            "collections" => format!(
                "{}/custom_collections.json?limit={}",
                self.base_url(),
                limit
            ),
            "variants" => format!("{}/variants.json?limit={}", self.base_url(), limit),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Shopify object: {obj}"
                )));
            }
        };
        let resp = self
            .client()
            .get(&path)
            .header("X-Shopify-Access-Token", &token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Shopify list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[obj].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        // Shopify expects a wrapper object (e.g. `{ "product": { ... } }`).
        let (path, key) = match obj {
            "products" => (format!("{}/products.json", self.base_url()), "product"),
            "orders" => (format!("{}/orders.json", self.base_url()), "order"),
            "customers" => (format!("{}/customers.json", self.base_url()), "customer"),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Shopify object: {obj}"
                )));
            }
        };
        let body = serde_json::json!({ key: data });
        let resp = self
            .client()
            .post(&path)
            .header("X-Shopify-Access-Token", &token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Shopify create failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[key].clone())
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        let (path, key) = match obj {
            "products" => (
                format!("{}/products/{}.json", self.base_url(), id),
                "product",
            ),
            "orders" => (format!("{}/orders/{}.json", self.base_url(), id), "order"),
            "customers" => (
                format!("{}/customers/{}.json", self.base_url(), id),
                "customer",
            ),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Shopify object: {obj}"
                )));
            }
        };
        let body = serde_json::json!({ key: data });
        let resp = self
            .client()
            .put(&path)
            .header("X-Shopify-Access-Token", &token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Shopify update failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json[key].clone())
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let token = self.token()?;
        let path = match obj {
            "products" => format!("{}/products/{}.json", self.base_url(), id),
            "customers" => format!("{}/customers/{}.json", self.base_url(), id),
            _ => return Err(ClawzError::Provider(format!("Cannot delete Shopify {obj}"))),
        };
        let resp = self
            .client()
            .delete(&path)
            .header("X-Shopify-Access-Token", &token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Shopify delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Shopify delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let token = self.token()?;
        match action {
            "fulfill_order" => {
                let order_id = params["order_id"].as_str().unwrap_or("");
                let resp = self
                    .client()
                    .post(format!(
                        "{}/orders/{}/fulfillments.json",
                        self.base_url(),
                        order_id
                    ))
                    .header("X-Shopify-Access-Token", &token)
                    .json(&serde_json::json!({ "fulfillment": params }))
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("Shopify fulfill order failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            "cancel_order" => {
                let order_id = params["order_id"].as_str().unwrap_or("");
                let resp = self
                    .client()
                    .post(format!(
                        "{}/orders/{}/cancel.json",
                        self.base_url(),
                        order_id
                    ))
                    .header("X-Shopify-Access-Token", &token)
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("Shopify cancel order failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            "search_products" => {
                let query = params["query"].as_str().unwrap_or("");
                let resp = self
                    .client()
                    .get(format!("{}/products.json?title={}", self.base_url(), query))
                    .header("X-Shopify-Access-Token", &token)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Shopify search failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Shopify action: {action}"
            ))),
        }
    }
}
