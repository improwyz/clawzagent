//! System-wide REST API — health, metrics, auth, config, and OpenAPI spec.
//!
//! This module exposes operational endpoints that are not tied to a specific
//! business domain (agents, fleet, etc.). It is the primary integration surface
//! for monitoring, identity, and API discovery.
//!
//! # Key responsibilities
//! | Endpoint              | Purpose                                           |
//! |-----------------------|---------------------------------------------------|
//! | `GET /system/health`  | Liveness / readiness probe for orchestrators       |
//! | `GET /system/info`    | Human-readable version and fleet summary           |
//! | `GET /system/metrics` | Prometheus text-format exposition                  |
//! | `POST /system/auth/*` | Login, registration, WebAuthn stubs                |
//! | `GET /system/openapi` | Self-describing OpenAPI 3.1 JSON spec              |
//!
//! # Security notes
//! - Passwords are hashed with a simple FNV-derived function for demo purposes.
//!   Production **must** switch to bcrypt / Argon2 via the `sha2` crate or
//!   an external identity provider.
//! - JWT tokens are issued on successful login and are validated by the
//!   `crate::auth::jwt` helper used in downstream middleware.
//!
//! # Cross-module interactions
//! - `system_info` and `system_metrics` read `AppState.agents` and
//!   `AppState.fleet_nodes` to produce aggregate counters.
//! - `login` / `register` read and write `AppState.users` and `AppState.api_keys`.

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

// Dependency: ApiKeyRecord, AppState, GatewayError, UserRecord defined in crate root.
use crate::{ApiKeyRecord, AppState, GatewayError, UserRecord};
use crate::password::{hash_password, verify_password};
use crate::prism_check;
// Dependency: auth::jwt helper for token creation and validation.
use crate::auth::jwt;

/// Assemble the system sub-router.
///
/// Mounted by the gateway at `/api/v1/system`.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Health / info
        .route("/health", get(health))
        .route("/info", get(system_info))
        // Config management
        .route("/config", get(get_config).put(update_config))
        .route("/config/reset", post(reset_config))
        // Prometheus metrics
        .route("/metrics", get(system_metrics))
        // Authentication
        .route("/auth/login", post(login))
        .route("/auth/register", post(register))
        .route("/auth/webauthn", post(webauthn_auth))
        // OpenAPI spec
        .route("/openapi", get(openapi_spec))
        // Pairing stubs retained for mobile-app compatibility
        .route("/pairing", post(create_pairing).delete(delete_pairing))
        .route("/prism", get(prism_status))
}

// ─── Body types ───────────────────────────────────────────────────────────────

/// Request body for `POST /system/auth/login`.
#[derive(Debug, Deserialize)]
pub struct LoginBody {
    /// Registered email address (required).
    pub email: Option<String>,
    /// Plain-text password (required).
    pub password: Option<String>,
    /// Tenant scope for Postgres fallback lookup; defaults to `default_tenant()`.
    pub tenant_id: Option<String>,
}

/// Request body for `POST /system/auth/register`.
#[derive(Debug, Deserialize)]
pub struct RegisterBody {
    /// Email address to register (required).
    pub email: Option<String>,
    /// Plain-text password, minimum 8 characters (required).
    pub password: Option<String>,
    /// Desired role; defaults to "user".
    pub role: Option<String>,
    /// Tenant scope for persistence; defaults to `default_tenant()`.
    pub tenant_id: Option<String>,
}

/// Request body for `PUT /system/config`.
#[derive(Debug, Deserialize)]
pub struct UpdateConfigBody {
    /// Log verbosity level, e.g. "debug", "info", "warn", "error".
    pub log_level: Option<String>,
    /// Ceiling on the number of agents the gateway will accept.
    pub max_agents: Option<u32>,
    /// Whether the audit pipeline is active.
    pub enable_audit: Option<bool>,
}

// ─── Health / info ────────────────────────────────────────────────────────────

/// `GET /system/health` — liveness probe.
///
/// Returns "healthy" plus uptime in seconds. Suitable for Kubernetes
/// `livenessProbe` and `readinessProbe` configurations.
async fn health(State(state): State<AppState>) -> Json<Value> {
    let uptime_secs = (Utc::now() - state.start_time).num_seconds().max(0) as u64;
    Json(json!({
        "status": "healthy",
        "uptime_secs": uptime_secs,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// `GET /system/info` — human-readable system summary.
///
/// Includes version, uptime, and lightweight counts so dashboards can render
/// a status card without issuing multiple requests.
async fn system_info(State(state): State<AppState>) -> Json<Value> {
    let uptime_secs = (Utc::now() - state.start_time).num_seconds().max(0) as u64;
    let agents = state.agents.read().await;
    let nodes = state.fleet_nodes.read().await;

    Json(json!({
        "name": "clawz-gateway",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": uptime_secs,
        "stats": {
            "agents": agents.len(),
            "fleet_nodes": nodes.len(),
        },
        "started_at": state.start_time,
    }))
}

// ─── Config ─────────────────────────────────────────────────────────────────────

/// `GET /system/config` — return current gateway configuration.
///
/// In this in-memory implementation values are either sourced from
/// environment variables or hard-coded defaults. A persistent backend would
/// read from a configuration store instead.
async fn get_config() -> Json<Value> {
    Json(json!({
        "log_level": std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        "max_agents": 100,
        "enable_audit": true,
        "gateway_version": env!("CARGO_PKG_VERSION"),
    }))
}

/// `PUT /system/config` — accept configuration updates.
///
/// Currently echoes the body back because there is no persistent config store.
/// In production this would validate, persist, and hot-reload the changes.
async fn update_config(Json(body): Json<UpdateConfigBody>) -> Json<Value> {
    // In a real implementation this would persist to a KV store and notify
    // background tasks of the new limits.
    Json(json!({
        "updated": true,
        "config": {
            "log_level": body.log_level.unwrap_or_else(|| "info".to_string()),
            "max_agents": body.max_agents.unwrap_or(100),
            "enable_audit": body.enable_audit.unwrap_or(true),
        }
    }))
}

/// `POST /system/config/reset` — restore factory defaults.
async fn reset_config() -> Json<Value> {
    Json(json!({
        "reset": true,
        "config": {
            "log_level": "info",
            "max_agents": 100,
            "enable_audit": true,
        }
    }))
}

// ─── Metrics ──────────────────────────────────────────────────────────────────

/// `GET /system/metrics` — Prometheus text-format exposition.
///
/// Emits gauge / counter lines for agents, conversations, fleet nodes, and
/// audit entries so Prometheus (or any OpenMetrics scraper) can ingest them
/// without an additional exporter side-car.
async fn system_metrics(State(state): State<AppState>) -> String {
    let agents = state.agents.read().await;
    let conversations = state.conversations.read().await;
    let nodes = state.fleet_nodes.read().await;
    let audit = state.audit_log.read().await;

    let running_agents = agents.iter().filter(|a| {
        matches!(a.status, crate::AgentStatus::Running)
    }).count();

    // Prometheus text format — each metric has a HELP and TYPE line followed
    // by the value line so scrapers can auto-discover semantics.
    format!(
        "# HELP clawz_agents_total Total number of agents\n\
         # TYPE clawz_agents_total gauge\n\
         clawz_agents_total {}\n\
         # HELP clawz_agents_running Running agents\n\
         # TYPE clawz_agents_running gauge\n\
         clawz_agents_running {}\n\
         # HELP clawz_conversations_total Total conversations\n\
         # TYPE clawz_conversations_total gauge\n\
         clawz_conversations_total {}\n\
         # HELP clawz_fleet_nodes_total Fleet nodes\n\
         # TYPE clawz_fleet_nodes_total gauge\n\
         clawz_fleet_nodes_total {}\n\
         # HELP clawz_audit_entries_total Audit log entries\n\
         # TYPE clawz_audit_entries_total counter\n\
         clawz_audit_entries_total {}\n",
        agents.len(),
        running_agents,
        conversations.len(),
        nodes.len(),
        audit.len(),
    )
}

// ─── Auth ─────────────────────────────────────────────────────────────────────

/// `POST /system/auth/login` — authenticate a user and issue a JWT.
///
/// # Security warning
/// Password verification uses a simple FNV-derived hash (`sha256_hex`).
/// This is **not** cryptographically secure and exists only so the auth flow
/// can be demonstrated without requiring the bcrypt crate in the current build.
/// Production must replace this with bcrypt/Argon2.
async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<Value>, GatewayError> {
    let email = body.email.ok_or_else(|| {
        GatewayError::Unprocessable("field 'email' is required".to_string())
    })?;
    let password = body.password.ok_or_else(|| {
        GatewayError::Unprocessable("field 'password' is required".to_string())
    })?;
    let tenant_id = body
        .tenant_id
        .unwrap_or_else(crate::postgres_store::default_tenant);

    let user = {
        let users = state.users.read().await;
        users.iter().find(|u| u.email == email).cloned()
    };

    let user = match user {
        Some(u) => u,
        None => {
            let Some(ref pool) = state.db else {
                return Err(GatewayError::Unauthorized(
                    "Invalid email or password".to_string(),
                ));
            };
            let db_user = crate::postgres_store::find_user_by_email(pool, &tenant_id, &email)
                .await
                .map_err(|e| GatewayError::Internal(e.to_string()))?
                .ok_or_else(|| {
                    GatewayError::Unauthorized("Invalid email or password".to_string())
                })?;
            let mut users = state.users.write().await;
            if let Some(cached) = users.iter().find(|u| u.email == email) {
                cached.clone()
            } else {
                users.push(db_user.clone());
                db_user
            }
        }
    };

    if !verify_password(&password, &user.password_hash) {
        return Err(GatewayError::Unauthorized("Invalid email or password".to_string()));
    }

    let token = jwt::create_token_with_tenant(
        &user.id,
        &user.email,
        &user.role,
        Some(&tenant_id),
        &state.jwt_secret,
        24,
    )
    .map_err(|e| GatewayError::Internal(e.to_string()))?;

    state
        .append_audit(&user.id, "login", "user", &user.id, None)
        .await;

    Ok(Json(json!({
        "token": token,
        "user_id": user.id,
        "email": user.email,
        "role": user.role,
    })))
}

/// `POST /system/auth/register` — create a new user account and default API key.
///
/// Enforces an 8-character minimum password length and deduplicates on email.
/// On success, returns the raw API key **once**; it is hashed before storage
/// and cannot be recovered later.
async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let email = body.email.ok_or_else(|| {
        GatewayError::Unprocessable("field 'email' is required".to_string())
    })?;
    let password = body.password.ok_or_else(|| {
        GatewayError::Unprocessable("field 'password' is required".to_string())
    })?;

    if password.len() < 8 {
        return Err(GatewayError::Unprocessable(
            "password must be at least 8 characters".to_string(),
        ));
    }

    // Check for duplicate email before mutating state so the error path is cheap.
    {
        let users = state.users.read().await;
        if users.iter().any(|u| u.email == email) {
            return Err(GatewayError::Unprocessable(format!(
                "email '{}' is already registered",
                email
            )));
        }
    }

    let now = Utc::now();
    let user_id = Uuid::new_v4().to_string();
    let user = UserRecord {
        id: user_id.clone(),
        email: email.clone(),
        password_hash: hash_password(&password).map_err(GatewayError::Internal)?,
        role: body.role.unwrap_or_else(|| "user".to_string()),
        created_at: now,
        updated_at: now,
    };

    // Generate an initial API key so the user can start making authenticated
    // requests immediately without a separate key-creation flow.
    let raw_key = format!("clawz_{}", Uuid::new_v4().to_string().replace('-', ""));
    let api_key = ApiKeyRecord {
        id: Uuid::new_v4().to_string(),
        user_id: user_id.clone(),
        key_hash: hash_password(&raw_key).map_err(GatewayError::Internal)?,
        label: "default".to_string(),
        created_at: now,
    };

    state.users.write().await.push(user.clone());
    state.api_keys.write().await.push(api_key.clone());

    if let Some(ref pool) = state.db {
        let tenant_id = body
            .tenant_id
            .unwrap_or_else(crate::postgres_store::default_tenant);
        crate::postgres_store::persist_user(pool, &user, &tenant_id)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        crate::postgres_store::persist_api_key(pool, &api_key, &tenant_id)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    state
        .append_audit(&user_id, "register", "user", &user_id, None)
        .await;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "user_id": user.id,
            "email": user.email,
            "role": user.role,
            "api_key": raw_key,
            "api_key_id": api_key.id,
            "created_at": user.created_at,
        })),
    ))
}

/// `POST /system/auth/webauthn` — begin WebAuthn registration/authentication ceremony.
async fn webauthn_auth(Json(body): Json<Value>) -> Json<Value> {
    let email = body["email"].as_str().unwrap_or("user@clawz.local");
    let challenge = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        Uuid::new_v4().as_bytes(),
    );
    Json(json!({
        "status": "challenge_issued",
        "publicKey": {
            "challenge": challenge,
            "rp": { "name": "ClawZ", "id": "localhost" },
            "user": {
                "id": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, email.as_bytes()),
                "name": email,
                "displayName": email
            },
            "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
            "timeout": 60000,
            "attestation": "none"
        }
    }))
}

/// `GET /system/prism` — live PRISM-G dimension capability status.
async fn prism_status() -> Json<Value> {
    Json(prism_check::prism_status_json().await)
}

// ─── OpenAPI spec ─────────────────────────────────────────────────────────────

/// `GET /system/openapi` — self-describing OpenAPI 3.1 JSON document.
///
/// Hand-curated so SDK generators and API explorers have an accurate (if
/// minimal) schema without requiring the utoipa macro expansion at build time.
async fn openapi_spec() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "ClawZ Gateway API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "REST API for the ClawZ agent platform"
        },
        "servers": [
            { "url": "/api/v1", "description": "ClawZ Gateway" }
        ],
        "paths": {
            "/agents": { "get": { "summary": "List agents" }, "post": { "summary": "Create agent" } },
            "/agents/{id}": { "get": { "summary": "Get agent" }, "put": { "summary": "Update agent" }, "delete": { "summary": "Delete agent" } },
            "/agents/{id}/run": { "post": { "summary": "Run agent" } },
            "/agents/{id}/stop": { "post": { "summary": "Stop agent" } },
            "/agents/{id}/history": { "get": { "summary": "Get agent conversation history" } },
            "/conversations": { "get": { "summary": "List conversations" }, "post": { "summary": "Create conversation" } },
            "/channels": { "get": { "summary": "List channels" }, "post": { "summary": "Create channel" } },
            "/providers": { "get": { "summary": "List providers" }, "post": { "summary": "Create provider" } },
            "/tools": { "get": { "summary": "List tools" }, "post": { "summary": "Create tool" } },
            "/governance/policies": { "get": { "summary": "List policies" } },
            "/governance/audit": { "get": { "summary": "Get audit log" } },
            "/governance/trust/{agent_id}": { "get": { "summary": "Get trust score" } },
            "/governance/evaluate": { "post": { "summary": "Evaluate governance" } },
            "/fleet": { "get": { "summary": "List fleet nodes" } },
            "/fleet/mesh": { "get": { "summary": "Get mesh topology" } },
            "/fleet/deploy": { "post": { "summary": "Deploy agent to node" } },
            "/fleet/metrics": { "get": { "summary": "Fleet metrics" } },
            "/system/health": { "get": { "summary": "Health check" } },
            "/system/metrics": { "get": { "summary": "Prometheus metrics" } },
            "/system/auth/login": { "post": { "summary": "Login" } },
            "/system/auth/register": { "post": { "summary": "Register" } },
        }
    }))
}

// ─── Pairing (compat) ─────────────────────────────────────────────────────────

/// `POST /system/pairing` — generate a short-lived pairing code.
///
/// Legacy mobile-app onboarding helper. The code expires in 5 minutes.
async fn create_pairing() -> Json<Value> {
    Json(json!({
        "pairing_code": Uuid::new_v4().to_string(),
        "expires_in_secs": 300,
    }))
}

/// `DELETE /system/pairing` — revoke an active pairing code.
async fn delete_pairing() -> StatusCode {
    StatusCode::NO_CONTENT
}

