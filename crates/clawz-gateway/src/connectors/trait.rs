//! # SaaS Connector Trait Definitions
//!
//! This module defines the core abstractions used by every connector in the gateway:
//!
//! - [`SaaSConnector`] — the async trait all platforms implement.
//! - [`AuthType`] — supported authentication schemes.
//! - [`Credentials`] — token and key storage after authentication.
//! - [`Filters`] — standard pagination, search, and timestamp filtering.
//!
//! ## Design Notes
//!
//! The trait is object-safe (`Send + Sync`) so that heterogeneous connector types can be
//! stored in the same [`ConnectorRegistry`] (`Arc<dyn SaaSConnector>`).
//!
//! // Dependency: `async_trait` enables async methods in traits.
//! // Dependency: `clawz_core::error::Result` unifies error handling across connectors.
//! // Dependency: `serde_json::Value` is used as the generic payload type for object data.

use async_trait::async_trait;
use clawz_core::error::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Authentication types supported by SaaS connectors.
///
/// Each platform exposes exactly one primary auth type via [`SaaSConnector::auth_type`].
/// The gateway uses this to choose the correct authentication flow (OAuth2 redirect,
/// API key input form, etc.).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthType {
    /// OAuth 2.0 authorization code flow.
    OAuth2,
    /// Static API key passed in header or query parameter.
    ApiKey,
    /// HTTP Basic authentication.
    BasicAuth,
    /// Bearer token authentication.
    BearerToken,
}

/// Credentials exchanged after successful authentication.
///
/// Not all fields are populated for every [`AuthType`]. For example, `OAuth2` fills
/// `access_token` and `refresh_token`, while `ApiKey` fills `api_key`. The `extra`
/// map stores platform-specific values such as instance URLs or tenant IDs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    /// OAuth2 access token or API key value.
    pub access_token: Option<String>,
    /// OAuth2 refresh token.
    pub refresh_token: Option<String>,
    /// Token expiry timestamp (UTC).
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// API key for non-OAuth2 connectors.
    pub api_key: Option<String>,
    /// Basic auth username.
    pub username: Option<String>,
    /// Basic auth password.
    pub password: Option<String>,
    /// Extra credential fields (e.g., instance URL, tenant ID).
    pub extra: HashMap<String, String>,
}

/// Filters for listing objects.
///
/// Passed to [`SaaSConnector::list_objects`] to control pagination, ordering,
/// free-text search, and time-range filtering. Individual connectors may ignore
/// fields that the underlying platform does not support.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filters {
    /// Maximum number of results to return.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
    /// Field to order results by.
    pub order_by: Option<String>,
    /// Order direction (asc or desc).
    pub order_direction: Option<String>,
    /// Free-text search query.
    pub search: Option<String>,
    /// Field-specific filters.
    pub fields: HashMap<String, String>,
    /// Created-after timestamp.
    pub created_after: Option<chrono::DateTime<chrono::Utc>>,
    /// Created-before timestamp.
    pub created_before: Option<chrono::DateTime<chrono::Utc>>,
    /// Updated-after timestamp.
    pub updated_after: Option<chrono::DateTime<chrono::Utc>>,
}

/// Core trait implemented by every SaaS connector.
///
/// This trait defines the uniform interface the gateway uses to interact with
/// disparate SaaS platforms. All methods are async and fallible because they
/// perform network I/O.
#[async_trait]
pub trait SaaSConnector: Send + Sync {
    /// Unique platform identifier (e.g., "salesforce", "hubspot").
    fn platform_id(&self) -> &str;

    /// Human-readable platform name (e.g., "Salesforce").
    fn display_name(&self) -> &str;

    /// Authentication type required by this platform.
    fn auth_type(&self) -> AuthType;

    /// Build an OAuth2 authorization URL.
    ///
    /// Returns an error if the connector does not use OAuth2.
    async fn auth_url(&self, redirect: &str) -> Result<String>;

    /// Exchange an OAuth2 authorization code for credentials.
    ///
    /// Returns an error if the connector does not use OAuth2.
    async fn exchange_code(&self, code: &str) -> Result<Credentials>;

    /// List objects of a given type with optional filters.
    async fn list_objects(&self, obj: &str, filters: &Filters) -> Result<Vec<Value>>;

    /// Create a new object.
    async fn create_object(&self, obj: &str, data: Value) -> Result<Value>;

    /// Update an existing object.
    async fn update_object(&self, obj: &str, id: &str, data: Value) -> Result<Value>;

    /// Delete an object by ID.
    async fn delete_object(&self, obj: &str, id: &str) -> Result<()>;

    /// Execute a platform-specific action.
    async fn execute_action(&self, action: &str, params: Value) -> Result<Value>;
}
