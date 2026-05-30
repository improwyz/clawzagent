//! Background ingest API — manual subconscious tick.

use axum::{Json, Router, routing::post};
use clawz_services::dto::{SubconsciousTickRequest, SubconsciousTickResponse};

use crate::{AppState, GatewayError};

pub fn routes() -> Router<AppState> {
    Router::new().route("/subconscious", post(run_subconscious))
}

async fn run_subconscious(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(body): Json<SubconsciousTickRequest>,
) -> Result<Json<SubconsciousTickResponse>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;
    let resp = platform
        .execution
        .run_subconscious_tick(body)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(Json(resp))
}
