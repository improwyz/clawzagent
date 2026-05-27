//! Meta (Facebook) Ads connector for Clawz Gateway.
//!
//! Talks to the Meta Marketing API (Graph API v19.0). Supports campaign, ad-set and
//! ad CRUD as well as insight reports and campaign-status toggles.
//!
//! # Authentication
//! OAuth2 only. The `ad_account_id` (format: `act_123`) is required for most
//! operations because the Marketing API is scoped to an ad account.
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

/// Graph API version hard-coded for this connector.
///
/// Bumping this constant is the single place to upgrade the Marketing API version.
const GRAPH_VERSION: &str = "v19.0";

/// Meta (Facebook) Ads connector.
///
/// All requests target `https://graph.facebook.com/{GRAPH_VERSION}`. The access
/// token is appended as a query parameter because that is the canonical pattern
/// for the Marketing API.
pub struct MetaAdsConnector {
    /// OAuth2 flow configuration.
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange.
    credentials: Option<Credentials>,
    /// Default ad account ID (e.g. `act_123456789`).
    ad_account_id: Option<String>,
}

impl MetaAdsConnector {
    /// Create a new Meta Ads connector.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        ad_account_id: Option<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://www.facebook.com/v19.0/dialog/oauth",
            "https://graph.facebook.com/v19.0/oauth/access_token",
            vec!["ads_read".into(), "ads_management".into()],
        );
        Self {
            oauth,
            credentials: None,
            ad_account_id,
        }
    }

    fn token(&self) -> Result<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.clone())
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))
    }

    fn account_id(&self) -> Result<String> {
        self.ad_account_id
            .clone()
            .ok_or_else(|| ClawzError::Provider("ad_account_id required".into()))
    }

    fn base_url() -> String {
        format!("https://graph.facebook.com/{}", GRAPH_VERSION)
    }
}

#[async_trait]
impl SaaSConnector for MetaAdsConnector {
    fn platform_id(&self) -> &str {
        "meta_ads"
    }

    fn display_name(&self) -> &str {
        "Meta Ads"
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
        let account_id = self.account_id()?;
        // Meta’s maximum page size for most edges is 200.
        let limit = filters.limit.unwrap_or(50).min(200);
        let path = match obj {
            "campaigns" => format!(
                "/act_{}/campaigns?fields=id,name,status,objective&limit={}",
                account_id, limit
            ),
            "adsets" => format!(
                "/act_{}/adsets?fields=id,name,status,campaign_id&limit={}",
                account_id, limit
            ),
            "ads" => format!(
                "/act_{}/ads?fields=id,name,status,adset_id&limit={}",
                account_id, limit
            ),
            "insights" => format!(
                "/act_{}/insights?fields=impressions,clicks,spend,reach&limit={}",
                account_id, limit
            ),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Meta Ads object: {obj}"
                )));
            }
        };
        let resp = reqwest::Client::new()
            .get(format!(
                "{}{}&access_token={}",
                Self::base_url(),
                path,
                token
            ))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Meta Ads list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        Ok(json["data"].as_array().cloned().unwrap_or_default())
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        let account_id = self.account_id()?;
        let path = match obj {
            "campaigns" => format!("/act_{}/campaigns", account_id),
            "adsets" => format!("/act_{}/adsets", account_id),
            "ads" => format!("/act_{}/ads", account_id),
            _ => {
                return Err(ClawzError::Provider(format!(
                    "Unknown Meta Ads object: {obj}"
                )));
            }
        };
        let resp = reqwest::Client::new()
            .post(format!("{}{}", Self::base_url(), path))
            .query(&[("access_token", &token)])
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Meta Ads create failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn update_object(&self, _obj: &str, id: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        let resp = reqwest::Client::new()
            .post(format!("{}/{}", Self::base_url(), id))
            .query(&[("access_token", &token)])
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Meta Ads update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, _obj: &str, id: &str) -> Result<()> {
        let token = self.token()?;
        let resp = reqwest::Client::new()
            .delete(format!("{}/{}", Self::base_url(), id))
            .query(&[("access_token", &token)])
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Meta Ads delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Meta Ads delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let token = self.token()?;
        let account_id = self.account_id()?;
        match action {
            "get_insights" => {
                let id = params["id"].as_str().unwrap_or(&account_id);
                let fields = params["fields"]
                    .as_str()
                    .unwrap_or("impressions,clicks,spend,reach,cpm,cpc");
                let resp = reqwest::Client::new()
                    .get(format!(
                        "{}/{}/insights?fields={}&access_token={}",
                        Self::base_url(),
                        id,
                        fields,
                        token
                    ))
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Meta Ads insights failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "pause_campaign" | "activate_campaign" => {
                let id = params["id"].as_str().unwrap_or("");
                let status = if action == "pause_campaign" {
                    "PAUSED"
                } else {
                    "ACTIVE"
                };
                let resp = reqwest::Client::new()
                    .post(format!("{}/{}", Self::base_url(), id))
                    .query(&[("access_token", &token)])
                    .json(&serde_json::json!({ "status": status }))
                    .send()
                    .await
                    .map_err(|e| {
                        ClawzError::Provider(format!("Meta Ads update status failed: {e}"))
                    })?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown Meta Ads action: {action}"
            ))),
        }
    }
}
