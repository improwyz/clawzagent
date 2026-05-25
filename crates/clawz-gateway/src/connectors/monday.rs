//! Monday.com connector for Clawz Gateway.
//!
//! Implements a GraphQL-based connector for Monday.com. Authentication is via an
//! API key only — Monday.com does not support OAuth2 for third-party integrations.
//!
//! # Supported objects
//! - `boards`, `items`, `workspaces`, `columns`
//!
//! # Supported actions
//! - `move_item_to_group`, `duplicate_board`
//!
//! # Cross-module dependencies
//! - [`SaaSConnector`] trait from `crate::connectors::r#trait`.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;

// Dependency: trait::{AuthType, Credentials, Filters, SaaSConnector}
use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// Monday.com connector (GraphQL API).
///
/// All communication happens over a single endpoint (`https://api.monday.com/v2`)
/// using the 2024-01 API version header.
pub struct MondayConnector {
    /// Monday.com API key (v2 token).
    api_key: String,
    /// Shared HTTP client.
    client: Client,
}

impl MondayConnector {
    /// Create a new Monday.com connector with an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Client::new(),
        }
    }

    /// Execute a GraphQL query against Monday.com.
    ///
    /// Automatically surfaces GraphQL errors as [`ClawzError::Provider`] so the
    /// gateway layer does not have to inspect the raw JSON.
    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self.client
            .post("https://api.monday.com/v2")
            .header("Authorization", &self.api_key)
            .header("API-Version", "2024-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Monday.com GraphQL failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        if let Some(errors) = json.get("errors") {
            return Err(ClawzError::Provider(format!("Monday.com GraphQL errors: {}", errors)));
        }
        Ok(json["data"].clone())
    }
}

#[async_trait]
impl SaaSConnector for MondayConnector {
    fn platform_id(&self) -> &str {
        "monday"
    }

    fn display_name(&self) -> &str {
        "Monday.com"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth("Monday.com uses API key authentication".into()))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth("Monday.com uses API key authentication".into()))
    }

    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>> {
        // Monday caps most `limit` arguments at 500.
        let limit = filters.limit.unwrap_or(50).min(500);
        match obj {
            "boards" => {
                let query = r#"query($limit: Int) { boards(limit: $limit) { id name description } }"#;
                let data = self.graphql(query, serde_json::json!({ "limit": limit })).await?;
                Ok(data["boards"].as_array().cloned().unwrap_or_default())
            }
            "items" => {
                let board_id = filters.search.as_deref().unwrap_or("");
                let query = r#"query($boardId: [ID!], $limit: Int) { boards(ids: $boardId) { items_page(limit: $limit) { items { id name state } } } }"#;
                let data = self.graphql(query, serde_json::json!({ "boardId": [board_id], "limit": limit })).await?;
                let items = data["boards"][0]["items_page"]["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                Ok(items)
            }
            "workspaces" => {
                let query = r#"query { workspaces { id name kind } }"#;
                let data = self.graphql(query, serde_json::json!({})).await?;
                Ok(data["workspaces"].as_array().cloned().unwrap_or_default())
            }
            _ => Err(ClawzError::Provider(format!("Unknown Monday.com object: {obj}"))),
        }
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        match obj {
            "boards" => {
                let name = data["name"].as_str().unwrap_or("New Board");
                let query = r#"mutation($name: String!) { create_board(board_name: $name, board_kind: public) { id name } }"#;
                let result = self.graphql(query, serde_json::json!({ "name": name })).await?;
                Ok(result["create_board"].clone())
            }
            "items" => {
                let board_id = data["board_id"].as_str().unwrap_or("");
                let name = data["name"].as_str().unwrap_or("New Item");
                let query = r#"mutation($boardId: ID!, $itemName: String!) { create_item(board_id: $boardId, item_name: $itemName) { id name } }"#;
                let result = self.graphql(query, serde_json::json!({ "boardId": board_id, "itemName": name })).await?;
                Ok(result["create_item"].clone())
            }
            "columns" => {
                let board_id = data["board_id"].as_str().unwrap_or("");
                let title = data["title"].as_str().unwrap_or("New Column");
                let column_type = data["column_type"].as_str().unwrap_or("text");
                let query = r#"mutation($boardId: ID!, $title: String!, $columnType: ColumnType!) { create_column(board_id: $boardId, title: $title, column_type: $columnType) { id title } }"#;
                let result = self.graphql(query, serde_json::json!({ "boardId": board_id, "title": title, "columnType": column_type })).await?;
                Ok(result["create_column"].clone())
            }
            _ => Err(ClawzError::Provider(format!("Unknown Monday.com object: {obj}"))),
        }
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        match obj {
            "items" => {
                let board_id = data["board_id"].as_str().unwrap_or("");
                let column_id = data["column_id"].as_str().unwrap_or("");
                let value = data["value"].to_string();
                let query = r#"mutation($boardId: ID!, $itemId: ID!, $columnId: String!, $value: JSON!) { change_column_value(board_id: $boardId, item_id: $itemId, column_id: $columnId, value: $value) { id } }"#;
                let result = self.graphql(query, serde_json::json!({ "boardId": board_id, "itemId": id, "columnId": column_id, "value": value })).await?;
                Ok(result["change_column_value"].clone())
            }
            _ => Err(ClawzError::Provider(format!("Unknown Monday.com object: {obj}"))),
        }
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let (mutation, key) = match obj {
            "boards" => (r#"mutation($id: ID!) { delete_board(board_id: $id) { id } }"#, "delete_board"),
            "items" => (r#"mutation($id: ID!) { delete_item(item_id: $id) { id } }"#, "delete_item"),
            _ => return Err(ClawzError::Provider(format!("Unknown Monday.com object: {obj}"))),
        };
        self.graphql(mutation, serde_json::json!({ "id": id })).await?;
        Ok(())
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        let query = match action {
            "move_item_to_group" => r#"mutation($itemId: ID!, $groupId: String!) { move_item_to_group(item_id: $itemId, group_id: $groupId) { id } }"#,
            "duplicate_board" => r#"mutation($boardId: ID!) { duplicate_board(board_id: $boardId, duplicate_type: duplicate_board_with_structure) { board { id name } } }"#,
            _ => return Err(ClawzError::Provider(format!("Unknown Monday.com action: {action}"))),
        };
        self.graphql(query, params).await
    }
}
