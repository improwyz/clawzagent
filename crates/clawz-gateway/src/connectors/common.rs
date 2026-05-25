//! # Common Connector Utilities
//!
//! Shared infrastructure used by most connector implementations. This module provides:
//!
//! - [`OAuth2Flow`] — a thin, reusable wrapper around the standard OAuth2 authorization-code
//!   grant, including token exchange and refresh.
//! - [`ApiClient`] — an HTTP client helper that pre-configures authentication headers based on
//!   the stored [`Credentials`] type (Bearer, API key, or Basic auth).
//! - [`TokenResponse`] — deserialization target for OAuth2 token endpoints.
//! - [`parse_json`] — convenience helper that turns HTTP responses into typed JSON or structured
//!   [`ClawzError`] variants.
//!
//! ## Why these helpers exist
//!
//! Instead of duplicating OAuth2 parameter building, header injection, and error mapping in
//! every connector, we centralize them here so individual modules stay focused on platform
//! API semantics.
//!
//! // Dependency: `crate::connectors::trait::Credentials` is consumed and produced by `OAuth2Flow`.
//! // Dependency: `clawz_core::error::{ClawzError, Result}` for uniform error propagation.

use crate::connectors::r#trait::Credentials;
use clawz_core::error::{ClawzError, Result};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Standard OAuth2 authorization-code flow helper.
///
/// Holds the client configuration (ID, secret, endpoints, scopes) required to build
/// authorization URLs and exchange codes for tokens. This struct is intentionally
/// cloneable so connectors can mutate the `redirect_uri` per-request without affecting
/// the original configuration.
#[derive(Debug, Clone)]
pub struct OAuth2Flow {
    /// OAuth2 client ID registered with the provider.
    pub client_id: String,
    /// OAuth2 client secret registered with the provider.
    pub client_secret: String,
    /// Redirect URI the provider will send the user back to after authorization.
    pub redirect_uri: String,
    /// Provider authorization endpoint URL.
    pub auth_url: String,
    /// Provider token endpoint URL.
    pub token_url: String,
    /// Requested OAuth2 scopes (space-separated when serialized).
    pub scopes: Vec<String>,
}

impl OAuth2Flow {
    /// Create a new OAuth2 flow configuration.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        auth_url: impl Into<String>,
        token_url: impl Into<String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect_uri: redirect_uri.into(),
            auth_url: auth_url.into(),
            token_url: token_url.into(),
            scopes,
        }
    }

    /// Build the authorization URL with state.
    ///
    /// Appends `response_type=code`, `client_id`, `redirect_uri`, `scope`, and the
    /// provided `state` to the provider's authorization endpoint. The state should be
    /// a cryptographically random value used to prevent CSRF attacks.
    pub fn auth_url(&self, state: &str) -> String {
        let mut url = Url::parse(&self.auth_url).expect("invalid auth_url");
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("response_type", "code");
            qp.append_pair("client_id", &self.client_id);
            qp.append_pair("redirect_uri", &self.redirect_uri);
            qp.append_pair("scope", &self.scopes.join(" "));
            qp.append_pair("state", state);
        }
        url.to_string()
    }

    /// Exchange an authorization code for access and refresh tokens.
    ///
    /// Sends a `POST` to the token endpoint with `grant_type=authorization_code`.
    /// On success, converts the provider's JSON response into our internal [`Credentials`]
    /// representation, computing the `expires_at` timestamp from `expires_in`.
    pub async fn exchange(&self, code: &str) -> Result<Credentials> {
        let client = Client::new();
        let mut params = HashMap::new();
        params.insert("grant_type", "authorization_code");
        params.insert("code", code);
        params.insert("client_id", &self.client_id);
        params.insert("client_secret", &self.client_secret);
        params.insert("redirect_uri", &self.redirect_uri);

        let response = client
            .post(&self.token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Auth(format!("token request failed: {e}")))?;

        if !response.status().is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".into());
            return Err(ClawzError::Auth(format!("token exchange failed: {body}")));
        }

        let body = response
            .text()
            .await
            .map_err(|e| ClawzError::Internal(format!("read body failed: {e}")))?;
        let token_resp: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        // Convert expires_in (seconds) to an absolute UTC timestamp.
        let expires_at = token_resp
            .expires_in
            .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));

        Ok(Credentials {
            access_token: Some(token_resp.access_token),
            refresh_token: token_resp.refresh_token,
            expires_at,
            api_key: None,
            username: None,
            password: None,
            extra: HashMap::new(),
        })
    }

    /// Refresh an expired access token using a refresh token.
    ///
    /// Sends `grant_type=refresh_token` to the token endpoint. If the provider does not
    /// return a new refresh token, the original one is preserved so the caller can
    /// continue refreshing in the future.
    pub async fn refresh(&self, refresh_token: &str) -> Result<Credentials> {
        let client = Client::new();
        let mut params = HashMap::new();
        params.insert("grant_type", "refresh_token");
        params.insert("refresh_token", refresh_token);
        params.insert("client_id", &self.client_id);
        params.insert("client_secret", &self.client_secret);

        let response = client
            .post(&self.token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Auth(format!("refresh request failed: {e}")))?;

        if !response.status().is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".into());
            return Err(ClawzError::Auth(format!("token refresh failed: {body}")));
        }

        let body = response
            .text()
            .await
            .map_err(|e| ClawzError::Internal(format!("read body failed: {e}")))?;
        let token_resp: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        // Convert expires_in (seconds) to an absolute UTC timestamp.
        let expires_at = token_resp
            .expires_in
            .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));

        Ok(Credentials {
            access_token: Some(token_resp.access_token),
            // Preserve the old refresh token if the provider does not rotate it.
            refresh_token: token_resp.refresh_token.or_else(|| Some(refresh_token.into())),
            expires_at,
            api_key: None,
            username: None,
            password: None,
            extra: HashMap::new(),
        })
    }
}

/// Standard OAuth2 token response.
///
/// Deserialized from the JSON payload returned by provider token endpoints. Some providers
/// omit `refresh_token` or `expires_in` on refresh responses.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenResponse {
    /// The access token used to authorize subsequent API calls.
    pub access_token: String,
    /// Token type, typically "Bearer".
    pub token_type: String,
    /// Refresh token, present only during initial authorization or when rotated.
    pub refresh_token: Option<String>,
    /// Lifetime of the access token in seconds.
    pub expires_in: Option<u64>,
    /// Granted scope(s), may differ from the requested scope.
    pub scope: Option<String>,
}

/// HTTP client helper with common request builders.
///
/// Wraps a [`reqwest::Client`] and a base URL, automatically injecting the correct
/// authentication headers based on the stored [`Credentials`] variant. This removes
/// boilerplate from every connector method.
#[derive(Debug, Clone)]
pub struct ApiClient {
    /// Underlying async HTTP client.
    client: Client,
    /// Base URL prepended to all request paths.
    base_url: String,
    /// Credentials used to build auth headers.
    credentials: Credentials,
}

impl ApiClient {
    /// Create a new API client.
    pub fn new(base_url: impl Into<String>, credentials: Credentials) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            credentials,
        }
    }

    /// Build a GET request with authentication headers.
    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.get(&url);
        req = Self::add_auth(req, &self.credentials);
        req
    }

    /// Build a POST request with authentication headers.
    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.post(&url);
        req = Self::add_auth(req, &self.credentials);
        req
    }

    /// Build a PATCH request with authentication headers.
    pub fn patch(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.patch(&url);
        req = Self::add_auth(req, &self.credentials);
        req
    }

    /// Build a PUT request with authentication headers.
    pub fn put(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.put(&url);
        req = Self::add_auth(req, &self.credentials);
        req
    }

    /// Build a DELETE request with authentication headers.
    pub fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.delete(&url);
        req = Self::add_auth(req, &self.credentials);
        req
    }

    /// Attach the best available auth header to a request builder.
    ///
    /// Priority: Bearer token (`access_token`) > API key (sent as Bearer) > Basic auth.
    fn add_auth(req: reqwest::RequestBuilder, creds: &Credentials) -> reqwest::RequestBuilder {
        if let Some(token) = &creds.access_token {
            req.bearer_auth(token)
        } else if let Some(key) = &creds.api_key {
            // Some providers expect "Bearer <api_key>" even for non-OAuth keys.
            req.header("Authorization", format!("Bearer {key}"))
        } else if let (Some(user), Some(pass)) = (&creds.username, &creds.password) {
            req.basic_auth(user, Some(pass))
        } else {
            req
        }
    }
}

/// Parse a JSON response or return a structured error.
///
/// For 2xx statuses, the body is read as text and deserialized into `T`. For non-2xx
/// statuses, a [`ClawzError::Provider`] is returned containing the HTTP status code and
/// raw response body so callers can surface meaningful error messages to users.
pub async fn parse_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T> {
    let status = response.status();
    if status.is_success() {
        let body = response
            .text()
            .await
            .map_err(|e| ClawzError::Internal(format!("read body failed: {e}")))?;
        serde_json::from_str(&body).map_err(|e| ClawzError::Serialization(e.to_string()))
    } else {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "unknown error".into());
        Err(ClawzError::Provider(format!(
            "HTTP {}: {}",
            status.as_u16(),
            body
        )))
    }
}
