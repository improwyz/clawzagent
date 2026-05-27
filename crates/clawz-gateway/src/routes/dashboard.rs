//! JSON dashboard metrics consumed by the web UI.

use axum::{extract::State, routing::get, Json, Router};
use chrono::Utc;
use serde_json::{json, Value};

use crate::{AgentStatus, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/metrics", get(dashboard_metrics))
}

/// `GET /dashboard/metrics` — JSON metrics for the React dashboard.
async fn dashboard_metrics(State(state): State<AppState>) -> Json<Value> {
    let agents = state.agents.read().await;
    let conversations = state.conversations.read().await;
    let nodes = state.fleet_nodes.read().await;

    let active = agents
        .iter()
        .filter(|a| matches!(a.status, AgentStatus::Running))
        .count();

    Json(json!({
        "active_agents": active,
        "total_conversations": conversations.len(),
        "requests_per_min": 0,
        "avg_latency_ms": 0,
        "requests_over_time": [],
        "provider_distribution": [],
        "fleet_nodes": nodes.len(),
        "generated_at": Utc::now(),
    }))
}
