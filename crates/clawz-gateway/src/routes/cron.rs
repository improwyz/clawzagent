//! Cron job REST API — `GET/POST /api/v1/cron/jobs`, `POST /api/v1/cron/jobs/{id}/run`.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use clawz_services::dto::{CreateCronJobRequest, CronJobDto, CronRunResultDto};
use serde_json::{Value, json};

use crate::{AppState, GatewayError};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/{id}/run", post(run_job))
        .route("/jobs/{id}", axum::routing::delete(delete_job))
}

async fn list_jobs(State(state): State<AppState>) -> Result<Json<Value>, GatewayError> {
    let platform = platform(&state)?;
    let jobs = platform
        .execution
        .list_cron_jobs()
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(Json(json!({ "data": jobs, "total": jobs.len() })))
}

async fn create_job(
    State(state): State<AppState>,
    Json(body): Json<CreateCronJobRequest>,
) -> Result<(StatusCode, Json<CronJobDto>), GatewayError> {
    let platform = platform(&state)?;
    let job = platform
        .execution
        .create_cron_job(body)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(job)))
}

async fn delete_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let platform = platform(&state)?;
    platform
        .execution
        .delete_cron_job(&id)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn run_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CronRunResultDto>, GatewayError> {
    let platform = platform(&state)?;
    let result = platform
        .execution
        .run_cron_job(&id)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(Json(result))
}

fn platform(state: &AppState) -> Result<&std::sync::Arc<clawz_services::Platform>, GatewayError> {
    state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))
}
