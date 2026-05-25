//! Communication channel REST API.
//!
//! Channels represent inbound / outbound integrations (webhooks, Slack, email,
//! etc.). Each `ChannelRecord` stores a JSON `config` blob so the schema can
//! evolve without migration churn. All endpoints are straightforward CRUD plus
//! a `test` action that simulates a message send to verify connectivity.
//!
//! # Cross-module notes
//! - No direct dependency on other route modules; operates only on `AppState.channels`.

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

// Dependency: AppState, ChannelRecord, GatewayError defined in crate root.
use crate::{AppState, ChannelRecord, GatewayError};

/// Assemble the channel sub-router.
///
/// Mounted by the gateway at `/api/v1/channels`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_channels).post(create_channel))
        .route("/{id}", get(get_channel).put(update_channel).delete(delete_channel))
        .route("/{id}/test", post(test_channel))
}

// ─── Body types ───────────────────────────────────────────────────────────────

/// Request body for `POST /channels`.
#[derive(Debug, Deserialize)]
pub struct CreateChannelBody {
    /// Human-readable channel name (required).
    pub name: Option<String>,
    /// Channel kind discriminator: "slack", "webhook", "email", etc. (required).
    pub channel_type: Option<String>,
    /// Opaque JSON configuration object consumed by the channel adapter.
    pub config: Option<serde_json::Value>,
    /// Whether the channel is active. Defaults to `true`.
    pub enabled: Option<bool>,
}

/// Request body for `PUT /channels/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateChannelBody {
    /// New display name.
    pub name: Option<String>,
    /// New channel kind.
    pub channel_type: Option<String>,
    /// Replaced configuration blob.
    pub config: Option<serde_json::Value>,
    /// Enable / disable toggle.
    pub enabled: Option<bool>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /channels` — list every registered channel.
async fn list_channels(State(state): State<AppState>) -> Json<Value> {
    let channels = state.channels.read().await;
    Json(json!({ "data": *channels, "total": channels.len() }))
}

/// `POST /channels` — register a new channel.
///
/// Requires `name` and `channel_type`. `config` defaults to an empty object
/// and `enabled` defaults to `true` so the channel is usable immediately.
async fn create_channel(
    State(state): State<AppState>,
    Json(body): Json<CreateChannelBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let name = body.name.ok_or_else(|| {
        GatewayError::Unprocessable("field 'name' is required".to_string())
    })?;
    let channel_type = body.channel_type.ok_or_else(|| {
        GatewayError::Unprocessable("field 'channel_type' is required".to_string())
    })?;

    let now = Utc::now();
    let record = ChannelRecord {
        id: Uuid::new_v4().to_string(),
        name,
        channel_type,
        config: body.config.unwrap_or(json!({})),
        enabled: body.enabled.unwrap_or(true),
        created_at: now,
        updated_at: now,
    };

    let mut channels = state.channels.write().await;
    channels.push(record.clone());
    Ok((StatusCode::CREATED, Json(json!(record))))
}

/// `GET /channels/{id}` — fetch a single channel.
async fn get_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let channels = state.channels.read().await;
    let record = channels
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Channel", &id))?;
    Ok(Json(json!(record)))
}

/// `PUT /channels/{id}` — apply a partial update.
///
/// Only supplied fields are overwritten; omitted fields keep their current values.
async fn update_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateChannelBody>,
) -> Result<Json<Value>, GatewayError> {
    let mut channels = state.channels.write().await;
    let record = channels
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Channel", &id))?;

    if let Some(name) = body.name { record.name = name; }
    if let Some(ct) = body.channel_type { record.channel_type = ct; }
    if let Some(cfg) = body.config { record.config = cfg; }
    if let Some(enabled) = body.enabled { record.enabled = enabled; }
    record.updated_at = Utc::now();

    Ok(Json(json!(record.clone())))
}

/// `DELETE /channels/{id}` — unregister a channel.
async fn delete_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mut channels = state.channels.write().await;
    let pos = channels
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Channel", &id))?;
    channels.remove(pos);
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /channels/{id}/test` — simulate a test message send.
///
/// If the channel is disabled, returns an early failure so callers do not
/// waste time debugging a known-bad configuration.
async fn test_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let channels = state.channels.read().await;
    let record = channels
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Channel", &id))?;

    if !record.enabled {
        return Ok(Json(json!({
            "id": id,
            "success": false,
            "message": "Channel is disabled",
        })));
    }

    // Simulate a test message send — in production this would delegate to the
    // channel adapter identified by `channel_type`.
    Ok(Json(json!({
        "id": id,
        "channel_type": record.channel_type,
        "success": true,
        "message": format!("Test message sent via {} channel '{}'", record.channel_type, record.name),
        "tested_at": Utc::now(),
    })))
}
