//! JSON dashboard metrics consumed by the web UI.

use axum::{Json, Router, extract::State, routing::get};
use chrono::Utc;
use serde_json::{Value, json};

use crate::{AgentStatus, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/metrics", get(dashboard_metrics))
}

/// Shared snapshot for `GET /dashboard/metrics` and `/ws/metrics`.
pub async fn snapshot_dashboard_metrics(state: &AppState) -> Value {
    let agents = state.agents.read().await;
    let conversations = state.conversations.read().await;
    let nodes = state.fleet_nodes.read().await;

    let active = agents
        .iter()
        .filter(|a| matches!(a.status, AgentStatus::Running))
        .count();

    json!({
        "active_agents": active,
        "total_conversations": conversations.len(),
        "requests_per_min": 0,
        "avg_latency_ms": 0,
        "requests_over_time": [],
        "provider_distribution": [],
        "fleet_nodes": nodes.len(),
        "generated_at": Utc::now().to_rfc3339(),
    })
}

/// `GET /dashboard/metrics` — JSON metrics for the React dashboard.
async fn dashboard_metrics(State(state): State<AppState>) -> Json<Value> {
    Json(snapshot_dashboard_metrics(&state).await)
}
