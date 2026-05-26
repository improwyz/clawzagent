//! Linear connector for Clawz Gateway.
//!
//! Implements a GraphQL-based connector for Linear (issue-tracking and project
//! management). Supports OAuth2 and API-key authentication.
//!
//! # Supported objects
//! - `issues`, `projects`, `cycles`, `teams`
//!
//! # Supported actions
//! - `archive_issue`, `search_issues`
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

/// Linear connector (GraphQL API).
///
/// Communicates exclusively over Linear’s single GraphQL endpoint. The connector
/// transparently handles both OAuth2 and API-key authentication by preferring the
/// access token and falling back to the API key.
pub struct LinearConnector {
    /// OAuth2 flow configuration (active when using the web-flow constructor).
    oauth: OAuth2Flow,
    /// Credentials after a successful OAuth exchange, or a manually injected API key.
    credentials: Option<Credentials>,
}

impl LinearConnector {
    /// Create a new Linear connector via OAuth2.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let oauth = OAuth2Flow::new(
            client_id,
            client_secret,
            redirect_uri,
            "https://linear.app/oauth/authorize",
            "https://api.linear.app/oauth/token",
            vec!["read".into(), "write".into(), "issues:create".into()],
        );
        Self { oauth, credentials: None }
    }

    /// Create with an API key (useful for internal integrations or CI bots).
    pub fn with_api_key(key: impl Into<String>) -> Self {
        let oauth = OAuth2Flow::new("", "", "", "", "", vec![]);
        Self {
            oauth,
            credentials: Some(Credentials {
                api_key: Some(key.into()),
                ..Default::default()
            }),
        }
    }

    /// Resolve the active token, preferring OAuth2 access token over API key.
    fn token(&self) -> Result<String> {
        self.credentials
            .as_ref()
            .and_then(|c| c.access_token.as_ref().or(c.api_key.as_ref()))
            .cloned()
            .ok_or_else(|| ClawzError::Auth("not authenticated".into()))
    }

    /// Execute a GraphQL query against `https://api.linear.app/graphql`.
    ///
    /// Automatically checks for `errors` in the response payload and returns them as
    /// a [`ClawzError::Provider`] so upstream callers receive a clean error.
    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let token = self.token()?;
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = reqwest::Client::new()
            .post("https://api.linear.app/graphql")
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Provider(format!("Linear GraphQL failed: {e}")))?;
        let json: Value = crate::connectors::common::parse_json(resp).await?;
        // Linear returns errors in a top-level "errors" array even for HTTP 200.
        if let Some(errors) = json.get("errors") {
            return Err(ClawzError::Provider(format!("Linear GraphQL errors: {}", errors)));
        }
        Ok(json["data"].clone())
    }
}

#[async_trait]
impl SaaSConnector for LinearConnector {
    fn platform_id(&self) -> &str {
        "linear"
    }

    fn display_name(&self) -> &str {
        "Linear"
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
        // Linear’s `first` argument is capped at 250.
        let first = filters.limit.unwrap_or(50).min(250);
        match obj {
            "issues" => {
                let query = r#"query($first: Int) { issues(first: $first) { nodes { id title state { name } priority } } }"#;
                let data = self.graphql(query, serde_json::json!({ "first": first })).await?;
                Ok(data["issues"]["nodes"].as_array().cloned().unwrap_or_default())
            }
            "projects" => {
                let query = r#"query($first: Int) { projects(first: $first) { nodes { id name state } } }"#;
                let data = self.graphql(query, serde_json::json!({ "first": first })).await?;
                Ok(data["projects"]["nodes"].as_array().cloned().unwrap_or_default())
            }
            "cycles" => {
                let team_id = filters.search.as_deref().unwrap_or("");
                let query = r#"query($teamId: String!, $first: Int) { team(id: $teamId) { cycles(first: $first) { nodes { id name startsAt endsAt } } } }"#;
                let data = self.graphql(query, serde_json::json!({ "teamId": team_id, "first": first })).await?;
                Ok(data["team"]["cycles"]["nodes"].as_array().cloned().unwrap_or_default())
            }
            "teams" => {
                let query = r#"query($first: Int) { teams(first: $first) { nodes { id name key } } }"#;
                let data = self.graphql(query, serde_json::json!({ "first": first })).await?;
                Ok(data["teams"]["nodes"].as_array().cloned().unwrap_or_default())
            }
            _ => Err(ClawzError::Provider(format!("Unknown Linear object: {obj}"))),
        }
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        match obj {
            "issues" => {
                let query = r#"mutation($input: IssueCreateInput!) { issueCreate(input: $input) { issue { id title } } }"#;
                let result = self.graphql(query, serde_json::json!({ "input": data })).await?;
                Ok(result["issueCreate"]["issue"].clone())
            }
            "projects" => {
                let query = r#"mutation($input: ProjectCreateInput!) { projectCreate(input: $input) { project { id name } } }"#;
                let result = self.graphql(query, serde_json::json!({ "input": data })).await?;
                Ok(result["projectCreate"]["project"].clone())
            }
            _ => Err(ClawzError::Provider(format!("Unknown Linear object: {obj}"))),
        }
    }

    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value> {
        match obj {
            "issues" => {
                let query = r#"mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { issue { id title state { name } } } }"#;
                let result = self.graphql(query, serde_json::json!({ "id": id, "input": data })).await?;
                Ok(result["issueUpdate"]["issue"].clone())
            }
            "projects" => {
                let query = r#"mutation($id: String!, $input: ProjectUpdateInput!) { projectUpdate(id: $id, input: $input) { project { id name } } }"#;
                let result = self.graphql(query, serde_json::json!({ "id": id, "input": data })).await?;
                Ok(result["projectUpdate"]["project"].clone())
            }
            _ => Err(ClawzError::Provider(format!("Unknown Linear object: {obj}"))),
        }
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        let (mutation, _key) = match obj {
            "issues" => (r#"mutation($id: String!) { issueDelete(id: $id) { success } }"#, "issueDelete"),
            "projects" => (r#"mutation($id: String!) { projectDelete(id: $id) { success } }"#, "projectDelete"),
            _ => return Err(ClawzError::Provider(format!("Unknown Linear object: {obj}"))),
        };
        self.graphql(mutation, serde_json::json!({ "id": id })).await?;
        Ok(())
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        match action {
            "archive_issue" => {
                let id = params["id"].as_str().unwrap_or("");
                let query = r#"mutation($id: String!) { issueArchive(id: $id) { success } }"#;
                self.graphql(query, serde_json::json!({ "id": id })).await
            }
            "search_issues" => {
                let term = params["term"].as_str().unwrap_or("");
                let query = r#"query($term: String!) { searchIssues(term: $term) { nodes { id title } } }"#;
                self.graphql(query, serde_json::json!({ "term": term })).await
            }
            _ => Err(ClawzError::Provider(format!("Unknown Linear action: {action}"))),
        }
    }
}
