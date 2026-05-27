//! Cloud provider deployment API (`/api/v1/cloud/*`).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::deploy::{DeployConfig, DeployMode, ProviderCredentials};
use crate::{AppState, GatewayError};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/providers", get(list_providers))
        .route("/validate", post(validate_provider_credentials))
        .route("/deploy", post(create_deployment))
        .route("/deployments", get(list_deployments))
        .route(
            "/deployments/{provider_id}/{deployment_id}",
            get(get_deployment_status).delete(destroy_deployment),
        )
}

#[derive(Debug, Deserialize)]
pub struct ValidateCredentialsBody {
    pub provider_id: String,
    pub credentials: ProviderCredentials,
}

#[derive(Debug, Deserialize)]
pub struct CloudDeployBody {
    pub provider_id: Option<String>,
    pub mode: Option<String>,
    pub image: Option<String>,
    pub env_vars: Option<std::collections::HashMap<String, String>>,
    pub region: Option<String>,
    pub replicas: Option<u32>,
    pub credentials: Option<ProviderCredentials>,
}

async fn list_providers(State(state): State<AppState>) -> Json<Value> {
    let infos = state.deploy_manager.provider_infos();
    Json(json!({ "data": infos }))
}

async fn validate_provider_credentials(
    State(state): State<AppState>,
    Json(body): Json<ValidateCredentialsBody>,
) -> Result<StatusCode, GatewayError> {
    state
        .deploy_manager
        .validate_config(&body.provider_id, &body.credentials)
        .await
        .map_err(|e| GatewayError::Unauthorized(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_deployment(
    State(state): State<AppState>,
    Json(body): Json<CloudDeployBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let mode = match body.mode.as_deref().unwrap_or("docker") {
        "native" | "native_binary" => DeployMode::NativeBinary,
        "wasm" => DeployMode::Wasm,
        _ => DeployMode::Docker {
            image: body
                .image
                .unwrap_or_else(|| "clawz/agent:latest".to_string()),
        },
    };

    let config = DeployConfig {
        mode,
        env_vars: body.env_vars.unwrap_or_default(),
        region: body.region,
        replicas: body.replicas.unwrap_or(1),
        credentials: body.credentials,
    };

    let provider_id = match body.provider_id {
        Some(id) => id,
        None => state
            .deploy_manager
            .auto_select_provider(&config)
            .map_err(|e| GatewayError::Unprocessable(e.to_string()))?
            .to_string(),
    };

    let info = state
        .deploy_manager
        .deploy(&provider_id, &config)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    state
        .append_audit("system", "cloud.deploy", "deployment", &info.id, None)
        .await;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "provider_id": provider_id,
            "deployment": info,
        })),
    ))
}

async fn list_deployments(State(state): State<AppState>) -> Result<Json<Value>, GatewayError> {
    let list = state
        .deploy_manager
        .list_deployments()
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(Json(json!({ "data": list, "total": list.len() })))
}

async fn get_deployment_status(
    State(state): State<AppState>,
    Path((provider_id, deployment_id)): Path<(String, String)>,
) -> Result<Json<Value>, GatewayError> {
    let status = state
        .deploy_manager
        .deployment_status(&provider_id, &deployment_id)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(Json(json!({
        "provider_id": provider_id,
        "deployment_id": deployment_id,
        "status": status,
    })))
}

async fn destroy_deployment(
    State(state): State<AppState>,
    Path((provider_id, deployment_id)): Path<(String, String)>,
) -> Result<StatusCode, GatewayError> {
    state
        .deploy_manager
        .destroy(&provider_id, &deployment_id)
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
