//! Session transcript management (list, compact, usage via worker).

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use clawz_services::dto::CompactSessionRequest;
use serde_json::json;

use crate::{AppState, GatewayError};

/// Mounted at `/api/v1/sessions`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_sessions))
        .route("/{id}/compact", post(compact_session))
}

async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let sessions = platform
        .execution
        .list_sessions()
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    Ok(Json(json!({
        "data": sessions,
        "total": sessions.len(),
    })))
}

async fn compact_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<CompactSessionRequest>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let resp = platform
        .execution
        .compact_session(&session_id, body)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    Ok(Json(json!({
        "session_id": resp.session_id,
        "removed": resp.removed,
        "message_count": resp.message_count,
        "estimated_tokens": resp.estimated_tokens,
    })))
}
