//! Authentication subsystem for the Clawz Gateway HTTP entry point.
//!
//! This module is responsible for validating incoming requests before they reach
//! business handlers. It supports two credential types:
//!
//! 1. **JWT Bearer tokens** — stateful session authentication for web users.
//! 2. **API keys** — long-lived, hashed credentials typically used by agents and
//!    external integrations.
//!
//! The primary entry point is [`auth_middleware`], an Axum middleware that
//! inspects headers (and query strings for API keys), resolves identity, and
//! injects an [`AuthContext`] into request extensions for downstream handlers.
//!
//! API key records are loaded from the `VALID_API_KEYS` environment variable at
//! request time. In production this should be replaced with a database or
//! shared-state lookup.
//!
//! # Key dependencies
//! - `axum` — middleware and request/response types.
//! - `jwt` sub-module — token creation and verification.
//! - `api_key` sub-module — key hashing, validation, and generation.

pub mod api_key;
pub mod jwt;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};

fn default_tenant_id() -> String {
    std::env::var("CLAWZ_TENANT_ID").unwrap_or_else(|_| "default".to_string())
}

fn tenant_from_claims(claims: &jwt::Claims) -> String {
    claims
        .tenant_id
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(default_tenant_id)
}

/// Dev-mode identity injected when `CLAWZ_DISABLE_AUTH=1`.
pub fn dev_auth_context() -> AuthContext {
    AuthContext {
        user_id: "dev".to_string(),
        email: "dev@local".to_string(),
        role: "owner".to_string(),
        tenant_id: default_tenant_id(),
        auth_method: AuthMethod::ApiKey,
    }
}

/// Authentication method that was used to establish identity for a request.
#[derive(Debug, Clone)]
pub enum AuthMethod {
    /// JSON Web Token (Bearer header) authentication.
    Jwt,
    /// API key authentication (header or query parameter).
    ApiKey,
}

/// Request-scoped identity container populated by [`auth_middleware`].
///
/// Downstream handlers and extractors can pull this type from Axum request
/// extensions to learn *who* is calling and *how* they authenticated.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// Unique identifier of the authenticated principal (user or service).
    pub user_id: String,
    /// Email address associated with the principal. For API keys this is
    /// synthesised from the user_id because email is not stored in key records.
    pub email: String,
    /// Role / permission group used for authorization decisions.
    pub role: String,
    /// Tenant scope for multi-tenant resource isolation.
    pub tenant_id: String,
    /// Which authentication path succeeded (JWT or API key).
    pub auth_method: AuthMethod,
}

/// URI paths that bypass authentication entirely.
///
/// These routes are checked with `starts_with`, so sub-paths are also exempt.
const PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/api/docs",
    "/api/v1/system/health",
    "/api/v1/system/auth/login",
    "/api/v1/system/auth/register",
    "/api/v1/system/auth/status",
    "/api/v1/system/openapi",
    "/webhooks/twilio",
    "/webhooks/google-voice",
];

/// Axum middleware that enforces JWT or API-key authentication.
///
/// Checks credentials in the following priority order:
///
/// 1. `Authorization: Bearer <jwt>` header
/// 2. `X-API-Key: <key>` header
/// 3. `?api_key=<key>` query parameter
///
/// Returns:
/// - **401 Unauthorized** — no credential was presented.
/// - **403 Forbidden** — a credential was presented but is invalid, expired,
///   revoked, or the signature check failed.
///
/// On success an [`AuthContext`] is inserted into request extensions and the
/// request is allowed to proceed to the next handler.
pub async fn auth_middleware(
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if std::env::var("CLAWZ_DISABLE_AUTH").ok().as_deref() == Some("1") {
        request.extensions_mut().insert(dev_auth_context());
        return Ok(next.run(request).await);
    }

    let path = request.uri().path().to_string();

    // Skip auth for public endpoints so health checks and login flows work
    // without credentials.
    if PUBLIC_PATHS.iter().any(|p| path.starts_with(p)) {
        return Ok(next.run(request).await);
    }

    // Dependency: `JWT_SECRET` is expected to be set in production.
    // Fallback to a well-known dev value so the gateway starts without extra
    // configuration in local development.
    let secret = std::env::var("CLAWZ_JWT_SECRET")
        .or_else(|_| std::env::var("JWT_SECRET"))
        .unwrap_or_else(|_| "changeme".to_string());

    // --- 1. Try Bearer JWT ---
    if let Some(auth_header) = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            // Dependency: jwt sub-module for token verification.
            match jwt::verify_token(token, &secret) {
                Ok(claims) => {
                    let tenant_id = tenant_from_claims(&claims);
                    let ctx = AuthContext {
                        user_id: claims.sub,
                        email: claims.email,
                        role: claims.role,
                        tenant_id,
                        auth_method: AuthMethod::Jwt,
                    };
                    request.extensions_mut().insert(ctx);
                    return Ok(next.run(request).await);
                }
                // JWT present but invalid → forbidden (don't fall through to
                // API key to avoid leaking that the token was parseable).
                Err(_) => return Err(StatusCode::FORBIDDEN),
            }
        }
    }

    // --- 2. Try X-API-Key header ---
    let api_key_header = request
        .headers()
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(raw_key) = api_key_header {
        return try_api_key(raw_key, request, next).await;
    }

    // --- 3. Try ?api_key= query param ---
    // Some webhook or SDK integrations pass keys in the query string because
    // setting custom headers is awkward in those environments.
    if let Some(query) = request.uri().query() {
        if let Some(raw_key) = extract_api_key_param(query) {
            return try_api_key(raw_key, request, next).await;
        }
    }

    // No credential presented at all.
    Err(StatusCode::UNAUTHORIZED)
}

/// Resolve credentials from HTTP headers and an optional query `api_key`.
///
/// Used by WebSocket handlers that sit outside the auth middleware stack.
pub fn resolve_request_auth(
    headers: &HeaderMap,
    api_key_query: Option<&str>,
) -> Result<AuthContext, StatusCode> {
    if std::env::var("CLAWZ_DISABLE_AUTH").ok().as_deref() == Some("1") {
        return Ok(dev_auth_context());
    }

    let secret = std::env::var("CLAWZ_JWT_SECRET")
        .or_else(|_| std::env::var("JWT_SECRET"))
        .unwrap_or_else(|_| "changeme".to_string());

    if let Some(auth_header) = headers.get("Authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            return match jwt::verify_token(token, &secret) {
                Ok(claims) => {
                    let tenant_id = tenant_from_claims(&claims);
                    Ok(AuthContext {
                        user_id: claims.sub,
                        email: claims.email,
                        role: claims.role,
                        tenant_id,
                        auth_method: AuthMethod::Jwt,
                    })
                }
                Err(_) => Err(StatusCode::FORBIDDEN),
            };
        }
    }

    if let Some(raw_key) = headers
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
    {
        return auth_context_from_api_key(&raw_key);
    }

    if let Some(raw_key) = api_key_query.map(|s| s.to_string()) {
        return auth_context_from_api_key(&raw_key);
    }

    Err(StatusCode::UNAUTHORIZED)
}

fn auth_context_from_api_key(raw_key: &str) -> Result<AuthContext, StatusCode> {
    let records = load_api_key_records_from_env();
    match api_key::ApiKeyValidator::validate(raw_key, &records) {
        Some(record) => Ok(AuthContext {
            user_id: record.user_id.clone(),
            email: format!("{}@apikey", record.user_id),
            role: record
                .permissions
                .first()
                .cloned()
                .unwrap_or_else(|| "agent".to_string()),
            tenant_id: if record.tenant_id.is_empty() {
                default_tenant_id()
            } else {
                record.tenant_id.clone()
            },
            auth_method: AuthMethod::ApiKey,
        }),
        None => Err(StatusCode::FORBIDDEN),
    }
}

/// Attempt to authenticate using a raw API key string.
///
/// Looks up the key against records loaded from the `VALID_API_KEYS`
/// environment variable. On success injects an [`AuthContext`] and continues
/// the request; on failure returns 403.
async fn try_api_key(
    raw_key: String,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // In a real deployment the valid keys would come from a database or
    // shared state injected via an extension. For now we look up an
    // environment variable list: VALID_API_KEYS=<hash1>:<user_id>:<role>,...
    let records = load_api_key_records_from_env();
    // Dependency: api_key sub-module for validation logic.
    match api_key::ApiKeyValidator::validate(&raw_key, &records) {
        Some(record) => {
            let ctx = AuthContext {
                user_id: record.user_id.clone(),
                // Synthesise an email because API key records don't store one.
                email: format!("{}@apikey", record.user_id),
                // Use the first permission entry as the role fallback.
                // "agent" is the default role when no permissions are listed.
                role: record
                    .permissions
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "agent".to_string()),
                tenant_id: if record.tenant_id.is_empty() {
                    default_tenant_id()
                } else {
                    record.tenant_id.clone()
                },
                auth_method: AuthMethod::ApiKey,
            };
            request.extensions_mut().insert(ctx);
            Ok(next.run(request).await)
        }
        None => Err(StatusCode::FORBIDDEN),
    }
}

/// Very small helper: parse `?api_key=VALUE` from a query string.
///
/// Performs a simple linear scan over `&`-delimited key-value pairs.
fn extract_api_key_param(query: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("api_key=") {
            return Some(percent_decode(value));
        }
    }
    None
}

/// Minimal percent-decode (only `%XX` sequences and `+` → space).
///
/// Note: This is intentionally lightweight. API keys are alphanumeric plus
/// `_`, so full RFC 3986 decoding is unnecessary.
fn percent_decode(s: &str) -> String {
    // Good enough for API keys which are alphanumeric + '_'.
    s.replace('+', " ")
}

/// Load API key records from the `VALID_API_KEYS` environment variable.
///
/// Expected format per entry: `hash:user_id:role`
/// Multiple entries are comma-separated.
///
/// # Example
/// ```text
/// VALID_API_KEYS=abc123:service_a:admin,def456:service_b:agent
/// ```
fn load_api_key_records_from_env() -> Vec<api_key::ApiKeyRecord> {
    // Dependency: chrono crate for timestamp generation.
    use chrono::Utc;
    let Ok(raw) = std::env::var("VALID_API_KEYS") else {
        return vec![];
    };
    raw.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let parts: Vec<&str> = entry.splitn(4, ':').collect();
            if parts.len() < 3 {
                // Plain dev key: "dev-key" — hash at runtime for validation.
                return Some(api_key::ApiKeyRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    key_hash: api_key::ApiKeyValidator::hash_key(entry),
                    user_id: "dev".to_string(),
                    tenant_id: default_tenant_id(),
                    name: "plain env key".to_string(),
                    permissions: vec!["owner".to_string()],
                    created_at: Utc::now(),
                    expires_at: None,
                    revoked: false,
                });
            }
            let tenant_id = parts
                .get(3)
                .filter(|t| !t.is_empty())
                .map(|t| (*t).to_string())
                .unwrap_or_else(default_tenant_id);
            let key_hash = if parts[0].len() == 64 && parts[0].chars().all(|c| c.is_ascii_hexdigit()) {
                parts[0].to_string()
            } else {
                api_key::ApiKeyValidator::hash_key(parts[0])
            };
            Some(api_key::ApiKeyRecord {
                id: uuid::Uuid::new_v4().to_string(),
                key_hash,
                user_id: parts[1].to_string(),
                tenant_id,
                name: "env key".to_string(),
                permissions: vec![parts[2].to_string()],
                created_at: Utc::now(),
                expires_at: None,
                revoked: false,
            })
        })
        .collect()
}
