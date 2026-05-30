//! HTTP control plane for the worker (`/v1/*`).

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use clawz_services::dto::{
    A2aInvokeRequest, A2aInvokeResponse, ChannelPollRequest, ChannelPollResponse,
    ChannelSendRequest, ChannelSendResponse, ChannelWebhookRequest, ChannelWebhookResponse,
    CompactSessionRequest, CompactSessionResponse, CreateCronJobRequest, CronJobDto,
    CronRunResultDto, EvaluateGovernanceRequest, EvaluateGovernanceResponse, ExecuteToolRequest,
    ExecuteToolResponse, FanOutRequest, FanOutResponse, MemoryIngestRequest, MemoryIngestResponse,
    OrchestrateRequest, OrchestrateResponse, ProviderHealthRequest, ProviderHealthResponse,
    RunTurnRequest, RunTurnResponse, SessionSummary, SubconsciousTickRequest,
    SubconsciousTickResponse,
    TestChannelRequest, TestChannelResponse,
};
use clawz_core::traits::AgentScheduler;
use clawz_core::types::orchestration::AgentSpec;
use clawz_core::types::tenant::{Role, TenantContext, TenantId};
use serde::Deserialize;
use serde_json::json;

use crate::orchestration::default_agent_spec;
use crate::service::WorkerService;

#[derive(Clone)]
pub struct ControlState {
    pub service: Arc<WorkerService>,
    pub agent_scheduler: Option<Arc<dyn AgentScheduler>>,
}

/// Expected bearer token from `WORKER_INTERNAL_TOKEN` or `CLAWZ_WORKER_TOKEN`.
fn worker_auth_token() -> Option<String> {
    std::env::var("WORKER_INTERNAL_TOKEN")
        .ok()
        .or_else(|| std::env::var("CLAWZ_WORKER_TOKEN").ok())
        .filter(|t| !t.is_empty())
}

async fn auth_middleware(request: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    if std::env::var("CLAWZ_DISABLE_AUTH").ok().as_deref() == Some("1") {
        return Ok(next.run(request).await);
    }

    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    let Some(expected) = worker_auth_token() else {
        tracing::error!(
            "worker control API auth enabled but neither WORKER_INTERNAL_TOKEN nor CLAWZ_WORKER_TOKEN is set"
        );
        return Err(StatusCode::UNAUTHORIZED);
    };

    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    let Some(token) = auth_header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    if token != expected {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(request).await)
}

pub fn routes(state: ControlState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/agents/{agent_id}/run", post(run_turn))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{session_id}/compact", post(compact_session))
        .route("/v1/tools/execute", post(execute_tool))
        .route("/v1/governance/evaluate", post(evaluate_governance))
        .route("/v1/providers/health", post(test_provider))
        .route("/v1/fanout", post(fan_out))
        .route("/v1/orchestrate", post(orchestrate))
        .route("/v1/a2a/invoke", post(a2a_invoke))
        .route("/v1/channels/test", post(test_channel))
        .route("/v1/channels/webhook", post(channel_webhook))
        .route("/v1/channels/send", post(channel_send))
        .route("/v1/channels/poll", post(channel_poll))
        .route("/v1/cron/jobs", get(list_cron_jobs).post(create_cron_job))
        .route("/v1/cron/jobs/{id}/run", post(run_cron_job))
        .route("/v1/cron/jobs/{id}", axum::routing::delete(delete_cron_job))
        .route("/v1/background/ingest", post(ingest_memory))
        .route("/v1/background/subconscious", post(run_subconscious))
        .route("/v1/fleet/spawn", post(fleet_spawn))
        .route("/v1/fleet/agents", get(fleet_list_agents))
        .layer(middleware::from_fn(auth_middleware))
        .with_state(state)
}

async fn health(State(state): State<ControlState>) -> Json<serde_json::Value> {
    let _ = state;
    Json(json!({ "status": "ok", "service": "clawz-worker" }))
}

async fn run_turn(
    State(state): State<ControlState>,
    Path(agent_id): Path<String>,
    Json(body): Json<RunTurnRequest>,
) -> Result<Json<RunTurnResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .run_turn(&agent_id, body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn list_sessions(
    State(state): State<ControlState>,
) -> Result<Json<Vec<SessionSummary>>, (axum::http::StatusCode, String)> {
    state
        .service
        .list_sessions()
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn compact_session(
    State(state): State<ControlState>,
    Path(session_id): Path<String>,
    Json(body): Json<CompactSessionRequest>,
) -> Result<Json<CompactSessionResponse>, (axum::http::StatusCode, String)> {
    let keep = body
        .keep_last
        .unwrap_or(crate::runtime::session_commands::DEFAULT_COMPACT_KEEP);
    let (removed, usage) = state
        .service
        .compact_session(&session_id, keep)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(CompactSessionResponse {
        session_id,
        removed,
        message_count: usage.message_count,
        estimated_tokens: usage.estimated_tokens,
    }))
}

async fn execute_tool(
    State(state): State<ControlState>,
    Json(body): Json<ExecuteToolRequest>,
) -> Result<Json<ExecuteToolResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .execute_tool(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn evaluate_governance(
    State(state): State<ControlState>,
    Json(body): Json<EvaluateGovernanceRequest>,
) -> Result<Json<EvaluateGovernanceResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .evaluate_governance(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn test_provider(
    State(state): State<ControlState>,
    Json(body): Json<ProviderHealthRequest>,
) -> Result<Json<ProviderHealthResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .test_provider(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn fan_out(
    State(state): State<ControlState>,
    Json(body): Json<FanOutRequest>,
) -> Result<Json<FanOutResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .fan_out(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn orchestrate(
    State(state): State<ControlState>,
    Json(body): Json<OrchestrateRequest>,
) -> Result<Json<OrchestrateResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .orchestrate(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn a2a_invoke(
    State(state): State<ControlState>,
    Json(body): Json<A2aInvokeRequest>,
) -> Result<Json<A2aInvokeResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .a2a_invoke(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn test_channel(
    State(state): State<ControlState>,
    Json(body): Json<TestChannelRequest>,
) -> Result<Json<TestChannelResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .test_channel(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn channel_webhook(
    State(state): State<ControlState>,
    Json(body): Json<ChannelWebhookRequest>,
) -> Result<Json<ChannelWebhookResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .process_channel_webhook(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn channel_send(
    State(state): State<ControlState>,
    Json(body): Json<ChannelSendRequest>,
) -> Result<Json<ChannelSendResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .send_channel_message(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn channel_poll(
    State(state): State<ControlState>,
    Json(body): Json<ChannelPollRequest>,
) -> Result<Json<ChannelPollResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .poll_channel(body)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn list_cron_jobs(
    State(state): State<ControlState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let jobs = state.service.list_cron_jobs().await.map_err(map_err)?;
    let data: Vec<CronJobDto> = jobs
        .into_iter()
        .map(crate::cron::convert::job_to_dto)
        .collect();
    Ok(Json(json!({ "data": data, "total": data.len() })))
}

async fn create_cron_job(
    State(state): State<ControlState>,
    Json(body): Json<CreateCronJobRequest>,
) -> Result<Json<CronJobDto>, (axum::http::StatusCode, String)> {
    let job = state
        .service
        .create_cron_job(crate::cron::convert::create_from_dto(body))
        .await
        .map_err(map_err)?;
    Ok(Json(crate::cron::convert::job_to_dto(job)))
}

async fn delete_cron_job(
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (axum::http::StatusCode, String)> {
    state.service.delete_cron_job(&id).await.map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn run_cron_job(
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> Result<Json<CronRunResultDto>, (axum::http::StatusCode, String)> {
    let result = state
        .service
        .execute_cron_job(&id)
        .await
        .map_err(map_err)?;
    Ok(Json(crate::cron::convert::run_to_dto(result)))
}

async fn ingest_memory(
    State(state): State<ControlState>,
    Json(body): Json<MemoryIngestRequest>,
) -> Result<Json<MemoryIngestResponse>, (axum::http::StatusCode, String)> {
    let chunks = body
        .chunks
        .into_iter()
        .map(|c| crate::background::MemoryIngestChunk {
            key: c.key,
            text: c.text,
            source: c.source,
        })
        .collect();
    let stored = state
        .service
        .ingest_memory(&body.agent_id, chunks)
        .await
        .map_err(map_err)?;
    Ok(Json(MemoryIngestResponse { stored }))
}

async fn run_subconscious(
    State(state): State<ControlState>,
    Json(body): Json<SubconsciousTickRequest>,
) -> Result<Json<SubconsciousTickResponse>, (axum::http::StatusCode, String)> {
    state
        .service
        .run_subconscious_tick(body.agent_id.as_deref())
        .await
        .map(Json)
        .map_err(map_err)
}

fn map_err(e: clawz_core::error::ClawzError) -> (axum::http::StatusCode, String) {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        e.to_string(),
    )
}

#[derive(Debug, Deserialize)]
struct FleetSpawnBody {
    tenant_id: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    spec: Option<AgentSpec>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

fn role_from_str(s: &str) -> Role {
    match s.to_lowercase().as_str() {
        "owner" => Role::Owner,
        "admin" => Role::Admin,
        "operator" => Role::Operator,
        "agent" => Role::Agent,
        "tool" => Role::Tool,
        _ => Role::Viewer,
    }
}

async fn fleet_spawn(
    State(state): State<ControlState>,
    Json(body): Json<FleetSpawnBody>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let scheduler = state
        .agent_scheduler
        .clone()
        .or_else(|| state.service.agent_scheduler())
        .ok_or((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "fleet scheduler not available (set CLAWZ_MODE=micro and mount docker.sock)"
                .to_string(),
        ))?;

    let ctx = TenantContext::new(
        TenantId::new(body.tenant_id),
        body.role
            .as_deref()
            .map(role_from_str)
            .unwrap_or(Role::Operator),
    );
    let mut spec = body.spec.unwrap_or_else(default_agent_spec);
    if let Some(caps) = body.capabilities {
        spec.capabilities = caps;
    }

    let handle = scheduler
        .spawn_agent(&ctx, spec)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "handle": handle })))
}

async fn fleet_list_agents(
    State(state): State<ControlState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let tenant_id = params.get("tenant_id").cloned().unwrap_or_else(|| {
        std::env::var("CLAWZ_TENANT_ID").unwrap_or_else(|_| "default".to_string())
    });

    let scheduler = state
        .agent_scheduler
        .clone()
        .or_else(|| state.service.agent_scheduler())
        .ok_or((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "fleet scheduler not available".to_string(),
        ))?;

    let agents = scheduler
        .list_agents(&tenant_id)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "agents": agents })))
}
