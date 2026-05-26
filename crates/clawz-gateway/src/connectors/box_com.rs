//! # Box Connector
//!
//! Integrates with the Box Content API (v2.0). Supports folders, files, collaborations,
//! shared links, and upload operations. Authentication is exclusively OAuth2.
//!
//! ## Supported Objects
//! - `files`, `folders`, `items`, `collaborations`, `search`
//! - `folder`, `collaboration`, `shared_link`, `file`
//!
//! ## Supported Actions
//! - `copy_file`, `move_file`, `get_file_info`
//!
//! // Dependency: `crate::connectors::common::OAuth2Flow` for token exchange.
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

use crate::connectors::common::OAuth2Flow;
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Box connector.
///
/// Stores an OAuth2 flow and optional credentials. Box separates its API into two
/// domains: `api.box.com` for metadata/RPC and `upload.box.com` for binary uploads.
#[allow(dead_code)]
pub struct BoxConnector {
    /// OAuth2 configuration including authorize and token endpoints.
    oauth: OAuth2Flow,
    /// Credentials after successful OAuth2 exchange.
    credentials: Option<Credentials>,
}

impl BoxConnector {
    /// Create a new Box connector via OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://account.box.com/api/oauth2/authorize",
            "https://api.box.com/oauth2/token",
            vec!["root_readwrite".into()],
        );
        Self { oauth, credentials: None }
    }

    /// Base URL for metadata and management calls.
    fn base_url(&self) -> &str {
        "https://api.box.com/2.0"
    }

    /// Base URL for file upload calls.
    fn upload_url(&self) -> &str {
        "https://upload.box.com/api/2.0"
    }

    /// Extract the access token from stored credentials.
    fn token(&self) -> Result<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.clone())
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))
    }

    /// Build a plain reqwest client for ad-hoc requests.
    fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }
}

#[async_trait]
impl SaaSConnector for BoxConnector {
    fn platform_id(&self) -> &str {
        "box"
    }

    fn display_name(&self) -> &str {
        "Box"
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
        let limit = filters.limit.unwrap_or(100).min(1000);
        // Default folder ID "0" is the user's root folder.
        let folder_id = filters.search.as_deref().unwrap_or("0");
        let path = match obj {
            "files" | "folders" | "items" => {
                format!("{}/folders/{}/items?limit={}", self.base_url(), folder_id, limit)
            }
            "collaborations" => format!("{}/folders/{}/collaborations", self.base_url(), folder_id),
            "search" => {
                let query = filters.search.as_deref().unwrap_or("");
                format!("{}/search?query={}&limit={}", self.base_url(), query, limit)
            }
            _ => return Err(ClawzError::Provider(format!("Unknown Box object: {obj}"))),
        };
        let resp = self.client()
            .get(&path)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Box list failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Box returns results under "entries" or "items" depending on endpoint.
        let entries = json["entries"].as_array().cloned()
            .or_else(|| json["items"].as_array().cloned())
            .unwrap_or_default();
        // Post-filter by type when the user asked for a specific subtype.
        if obj == "files" {
            Ok(entries.into_iter().filter(|e| e["type"].as_str() == Some("file")).collect())
        } else if obj == "folders" {
            Ok(entries.into_iter().filter(|e| e["type"].as_str() == Some("folder")).collect())
        } else {
            Ok(entries)
        }
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        match obj {
            "folder" => {
                let name = data["name"].as_str().unwrap_or("New Folder");
                let parent_id = data["parent_id"].as_str().unwrap_or("0");
                let body = serde_json::json!({
                    "name": name,
                    "parent": { "id": parent_id }
                });
                let resp = self.client()
                    .post(format!("{}/folders", self.base_url()))
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Box create folder failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "collaboration" => {
                let resp = self.client()
                    .post(format!("{}/collaborations", self.base_url()))
                    .bearer_auth(&token)
                    .json(&data)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Box create collaboration failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "shared_link" => {
                let item_type = data["item_type"].as_str().unwrap_or("file");
                let item_id = data["item_id"].as_str().unwrap_or("");
                let path = format!("{}s/{}", item_type, item_id);
                let body = serde_json::json!({
                    "shared_link": {
                        "access": data["access"].as_str().unwrap_or("open")
                    }
                });
                let resp = self.client()
                    .put(format!("{}/{}", self.base_url(), path))
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Box create shared link failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown Box object: {obj}"))),
        }
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        let path = match obj {
            "file" => format!("{}/files/{}", self.base_url(), id),
            "folder" => format!("{}/folders/{}", self.base_url(), id),
            _ => return Err(ClawzError::Provider(format!("Unknown Box object: {obj}"))),
        };
        let resp = self.client()
            .put(&path)
            .bearer_auth(&token)
            .json(&data)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Box update failed: {e}")))?;
        crate::connectors::common::parse_json(resp).await
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let token = self.token()?;
        let path = match obj {
            "file" => format!("{}/files/{}", self.base_url(), id),
            // `recursive=true` is required to delete non-empty folders.
            "folder" => format!("{}/folders/{}?recursive=true", self.base_url(), id),
            "collaboration" => format!("{}/collaborations/{}", self.base_url(), id),
            _ => return Err(ClawzError::Provider(format!("Unknown Box object: {obj}"))),
        };
        let resp = self.client()
            .delete(&path)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Box delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Box delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let token = self.token()?;
        match action {
            "copy_file" => {
                let file_id = params["file_id"].as_str().unwrap_or("");
                let resp = self.client()
                    .post(format!("{}/files/{}/copy", self.base_url(), file_id))
                    .bearer_auth(&token)
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Box copy file failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "move_file" => {
                let file_id = params["file_id"].as_str().unwrap_or("");
                let parent_id = params["parent_id"].as_str().unwrap_or("0");
                let body = serde_json::json!({ "parent": { "id": parent_id } });
                let resp = self.client()
                    .put(format!("{}/files/{}", self.base_url(), file_id))
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Box move file failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "get_file_info" => {
                let file_id = params["file_id"].as_str().unwrap_or("");
                let resp = self.client()
                    .get(format!("{}/files/{}", self.base_url(), file_id))
                    .bearer_auth(&token)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Box get file info failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown Box action: {action}"))),
        }
    }
}
