//! # Dropbox Connector
//!
//! Integrates with the Dropbox API (v2). Supports files, folders, shared links, and
//! upload operations. Uses OAuth2 for authentication and separates RPC calls
//! (`api.dropboxapi.com`) from content uploads (`content.dropboxapi.com`).
//!
//! ## Supported Objects
//! - `files`, `folders`, `shared_links`
//! - `folder`, `shared_link`, `file`
//!
//! ## Supported Actions
//! - `copy`, `get_metadata`, `search`
//!
//! // Dependency: `crate::connectors::common::OAuth2Flow` for token exchange.
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use serde_json::Value;

use crate::connectors::common::OAuth2Flow;
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Dropbox connector.
///
/// Stores OAuth2 flow state and optional credentials. Dropbox requires passing
/// arguments via JSON in the `Dropbox-API-Arg` header for content endpoints, while
/// RPC endpoints accept standard JSON bodies.
pub struct DropboxConnector {
    /// OAuth2 configuration for the Dropbox app.
    oauth: OAuth2Flow,
    /// Credentials after successful authorization.
    credentials: Option<Credentials>,
}

impl DropboxConnector {
    /// Create a new Dropbox connector via OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://www.dropbox.com/oauth2/authorize",
            "https://api.dropboxapi.com/oauth2/token",
            vec!["account_info.read".into(), "files.content.read".into(), "files.content.write".into()],
        );
        Self { oauth, credentials: None }
    }

    /// Extract the access token from stored credentials.
    fn token(&self) -> Result<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.clone())
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))
    }

    /// Build an RPC POST request to the metadata API.
    fn rpc_post(&self, endpoint: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("https://api.dropboxapi.com/2/{}", endpoint))
    }

    /// Build a content POST request to the upload/download API.
    fn content_post(&self, endpoint: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("https://content.dropboxapi.com/2/{}", endpoint))
    }
}

#[async_trait]
impl SaaSConnector for DropboxConnector {
    fn platform_id(&self) -> &str {
        "dropbox"
    }

    fn display_name(&self) -> &str {
        "Dropbox"
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
        let path = filters.search.as_deref().unwrap_or("");
        match obj {
            "files" | "folders" => {
                let body = serde_json::json!({ "path": path });
                let resp = self.rpc_post("files/list_folder")
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox list failed: {e}")))?;
                let json: Value = crate::connectors::common::parse_json(resp).await?;
                let entries: Vec<Value> = json["entries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|e| {
                        if obj == "folders" {
                            e[".tag"].as_str() == Some("folder")
                        } else {
                            e[".tag"].as_str() == Some("file")
                        }
                    })
                    .collect();
                Ok(entries)
            }
            "shared_links" => {
                let resp = self.rpc_post("sharing/list_shared_links")
                    .bearer_auth(&token)
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox list shared links failed: {e}")))?;
                let json: Value = crate::connectors::common::parse_json(resp).await?;
                Ok(json["links"].as_array().cloned().unwrap_or_default())
            }
            _ => Err(ClawzError::Provider(format!("Unknown Dropbox object: {obj}"))),
        }
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        match obj {
            "folder" => {
                let path = data["path"].as_str().unwrap_or("");
                let resp = self.rpc_post("files/create_folder_v2")
                    .bearer_auth(&token)
                    .json(&serde_json::json!({ "path": path, "autorename": false }))
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox create folder failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "shared_link" => {
                let path = data["path"].as_str().unwrap_or("");
                let resp = self.rpc_post("sharing/create_shared_link_with_settings")
                    .bearer_auth(&token)
                    .json(&serde_json::json!({ "path": path }))
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox create shared link failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "file" => {
                let path = data["path"].as_str().unwrap_or("/untitled");
                let content = data["content"].as_str().unwrap_or("");
                let api_arg = serde_json::json!({
                    "path": path,
                    "mode": "add",
                    "autorename": true
                });
                let resp = self.content_post("files/upload")
                    .bearer_auth(&token)
                    .header("Dropbox-API-Arg", api_arg.to_string())
                    .header("Content-Type", "application/octet-stream")
                    .body(content.to_string())
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox upload failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown Dropbox object: {obj}"))),
        }
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        let token = self.token()?;
        match obj {
            "file" => {
                // Dropbox "update" for files is implemented as a move/rename.
                let to_path = data["to_path"].as_str().unwrap_or(id);
                let resp = self.rpc_post("files/move_v2")
                    .bearer_auth(&token)
                    .json(&serde_json::json!({ "from_path": id, "to_path": to_path }))
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox move failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown Dropbox object: {obj}"))),
        }
    }

    async fn delete_object(&self, _obj: &str, id: &str) -> Result<()> {
        let token = self.token()?;
        let resp = self.rpc_post("files/delete_v2")
            .bearer_auth(&token)
            .json(&serde_json::json!({ "path": id }))
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Dropbox delete failed: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClawzError::Provider(format!(
                "Dropbox delete failed: HTTP {}",
                resp.status().as_u16()
            )))
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let token = self.token()?;
        match action {
            "copy" => {
                let resp = self.rpc_post("files/copy_v2")
                    .bearer_auth(&token)
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox copy failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "get_metadata" => {
                let resp = self.rpc_post("files/get_metadata")
                    .bearer_auth(&token)
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox get metadata failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            "search" => {
                let resp = self.rpc_post("files/search_v2")
                    .bearer_auth(&token)
                    .json(&params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("Dropbox search failed: {e}")))?;
                crate::connectors::common::parse_json(resp).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown Dropbox action: {action}"))),
        }
    }
}
