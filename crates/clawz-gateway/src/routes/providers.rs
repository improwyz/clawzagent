//! External provider REST API.
//!
//! Providers encapsulate third-party API endpoints — LLM backends, embedding
//! services, search engines, etc. Each record stores an optional `api_key` and
//! `base_url` so the gateway can route agent inference requests to the correct
//! upstream. The list endpoint masks `api_key` to avoid leaking secrets in
//! casual API browsing.
//!
//! # Security notes
//! - `api_key` is **not** returned by `GET /providers` or `GET /providers/{id}`.
//! - In production the key should be encrypted at rest and injected via a secret
//!   manager rather than stored in the gateway's in-memory state.
//!
//! # Cross-module interactions
//! - None directly; providers are consumed by the agent execution layer (not yet
//!   fully wired in the route handlers below).

use clawz_services::dto::ProviderHealthRequest;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

// Dependency: AppState, GatewayError, ProviderRecord defined in crate root.
use crate::{AppState, GatewayError, ProviderRecord};

/// Assemble the provider sub-router.
///
/// Mounted by the gateway at `/api/v1/providers`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_providers).post(create_provider))
        .route("/{id}", get(get_provider).put(update_provider).delete(delete_provider))
        .route("/{id}/test", post(test_provider))
        // Legacy health route kept for compatibility with older monitoring scripts.
        .route("/{id}/health", get(provider_health))
}

// ─── Body types ───────────────────────────────────────────────────────────────

/// Request body for `POST /providers`.
#[derive(Debug, Deserialize)]
pub struct CreateProviderBody {
    /// Human-readable provider name (required).
    pub name: Option<String>,
    /// Provider kind discriminator, e.g. "openai", "anthropic", "local" (required).
    pub provider_type: Option<String>,
    /// Secret API key. Stored but never echoed back in read endpoints.
    pub api_key: Option<String>,
    /// Base URL override for self-hosted or proxy deployments.
    pub base_url: Option<String>,
    /// Whether the provider is eligible for routing. Defaults to `true`.
    pub enabled: Option<bool>,
}

/// Request body for `PUT /providers/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateProviderBody {
    /// New display name.
    pub name: Option<String>,
    /// New provider kind.
    pub provider_type: Option<String>,
    /// Rotated API key.
    pub api_key: Option<String>,
    /// New base URL.
    pub base_url: Option<String>,
    /// Enable / disable toggle.
    pub enabled: Option<bool>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /providers` — list all providers with secrets masked.
///
/// The `api_key` field is deliberately omitted from the JSON projection so
/// listing the providers does not leak credentials into logs or browser dev-tools.
async fn list_providers(State(state): State<AppState>) -> Json<Value> {
    let providers = state.providers.read().await;
    let masked: Vec<Value> = providers.iter().map(|p| json!({
        "id": p.id,
        "name": p.name,
        "provider_type": p.provider_type,
        "base_url": p.base_url,
        "enabled": p.enabled,
        "created_at": p.created_at,
        "updated_at": p.updated_at,
    })).collect();
    Json(json!({ "data": masked, "total": masked.len() }))
}

/// `POST /providers` — register a new upstream provider.
///
/// The response also omits `api_key` so the one-time secret is only visible
/// in the POST response of the creation call (and even that should be handled
/// carefully in production).
async fn create_provider(
    State(state): State<AppState>,
    Json(body): Json<CreateProviderBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let name = body.name.ok_or_else(|| {
        GatewayError::Unprocessable("field 'name' is required".to_string())
    })?;
    let provider_type = body.provider_type.ok_or_else(|| {
        GatewayError::Unprocessable("field 'provider_type' is required".to_string())
    })?;

    let now = Utc::now();
    let record = ProviderRecord {
        id: Uuid::new_v4().to_string(),
        name,
        provider_type,
        api_key: body.api_key,
        base_url: body.base_url,
        enabled: body.enabled.unwrap_or(true),
        created_at: now,
        updated_at: now,
    };

    let mut providers = state.providers.write().await;
    providers.push(record.clone());
    if let Some(ref pool) = state.db {
        let _ = crate::postgres_store::persist_provider(pool, &record).await;
    }

    Ok((StatusCode::CREATED, Json(json!({
        "id": record.id,
        "name": record.name,
        "provider_type": record.provider_type,
        "base_url": record.base_url,
        "enabled": record.enabled,
        "created_at": record.created_at,
        "updated_at": record.updated_at,
    }))))
}

/// `GET /providers/{id}` — fetch a single provider (secret masked).
async fn get_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let providers = state.providers.read().await;
    let record = providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| GatewayError::not_found("Provider", &id))?;
    Ok(Json(json!({
        "id": record.id,
        "name": record.name,
        "provider_type": record.provider_type,
        "base_url": record.base_url,
        "enabled": record.enabled,
        "created_at": record.created_at,
        "updated_at": record.updated_at,
    })))
}

/// `PUT /providers/{id}` — partial update.
///
/// Only supplied fields are overwritten. If `api_key` is provided it replaces
/// the old one; there is no "patch" semantics for key rotation here.
async fn update_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProviderBody>,
) -> Result<Json<Value>, GatewayError> {
    let mut providers = state.providers.write().await;
    let record = providers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| GatewayError::not_found("Provider", &id))?;

    if let Some(name) = body.name { record.name = name; }
    if let Some(pt) = body.provider_type { record.provider_type = pt; }
    if let Some(key) = body.api_key { record.api_key = Some(key); }
    if let Some(url) = body.base_url { record.base_url = Some(url); }
    if let Some(enabled) = body.enabled { record.enabled = enabled; }
    record.updated_at = Utc::now();
    let snapshot = record.clone();
    drop(providers);
    if let Some(ref pool) = state.db {
        let _ = crate::postgres_store::persist_provider(pool, &snapshot).await;
    }

    Ok(Json(json!({
        "id": snapshot.id,
        "name": snapshot.name,
        "provider_type": snapshot.provider_type,
        "base_url": snapshot.base_url,
        "enabled": snapshot.enabled,
        "created_at": snapshot.created_at,
        "updated_at": snapshot.updated_at,
    })))
}

/// `DELETE /providers/{id}` — unregister a provider.
async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mut providers = state.providers.write().await;
    let pos = providers
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| GatewayError::not_found("Provider", &id))?;
    providers.remove(pos);
    if let Some(ref pool) = state.db {
        crate::postgres_store::delete_provider(pool, &id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /providers/{id}/test` — simulate a connectivity check.
///
/// If the provider is disabled we return immediately so the caller knows the
/// configuration is inactive rather than broken.
async fn test_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let providers = state.providers.read().await;
    let record = providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| GatewayError::not_found("Provider", &id))?;

    if !record.enabled {
        return Ok(Json(json!({
            "id": id,
            "healthy": false,
            "message": "Provider is disabled",
            "tested_at": Utc::now(),
        })));
    }

    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let health = platform
        .execution
        .test_provider(ProviderHealthRequest {
            provider_id: record.provider_type.clone(),
            endpoint: record.base_url.clone(),
            api_key: record.api_key.clone(),
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    Ok(Json(json!({
        "id": id,
        "provider_type": record.provider_type,
        "healthy": health.ok,
        "latency_ms": health.latency_ms,
        "message": health.message,
        "tested_at": Utc::now(),
    })))
}

/// `GET /providers/{id}/health` — legacy alias for `test_provider`.
///
/// Preserved so existing load-balancer health checks do not need to change
/// their HTTP method from GET to POST.
async fn provider_health(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    test_provider(State(state), Path(id)).await
}
