//! Aggregated dashboard overview: health, counts, and API surface catalog.

use axum::{Json, Router, extract::State, routing::get};
use chrono::Utc;
use serde_json::{Value, json};

use crate::{AgentStatus, AppState};

/// API groups exposed to the web dashboard (method, path, purpose).
fn api_catalog() -> Value {
    json!([
        {
            "group": "Monitoring",
            "items": [
                { "method": "GET", "path": "/health", "summary": "Liveness probe (no auth)" },
                { "method": "GET", "path": "/api/v1/dashboard/overview", "summary": "This overview payload" },
                { "method": "GET", "path": "/api/v1/dashboard/metrics", "summary": "Dashboard KPI JSON" },
                { "method": "GET", "path": "/api/v1/system/metrics", "summary": "Prometheus text metrics" },
                { "method": "GET", "path": "/api/v1/fleet/metrics", "summary": "Fleet health snapshot" },
                { "method": "WS", "path": "/ws/metrics", "summary": "Live metrics stream" },
                { "method": "WS", "path": "/ws/events", "summary": "Platform events" },
                { "method": "WS", "path": "/ws/logs", "summary": "Log tail stream" }
            ]
        },
        {
            "group": "Agents",
            "items": [
                { "method": "GET", "path": "/api/v1/agents", "summary": "List agents" },
                { "method": "POST", "path": "/api/v1/agents", "summary": "Create agent" },
                { "method": "GET", "path": "/api/v1/agents/{id}", "summary": "Get agent" },
                { "method": "PUT", "path": "/api/v1/agents/{id}", "summary": "Update agent" },
                { "method": "DELETE", "path": "/api/v1/agents/{id}", "summary": "Delete agent" },
                { "method": "POST", "path": "/api/v1/agents/{id}/run", "summary": "Run single turn" },
                { "method": "POST", "path": "/api/v1/agents/{id}/stop", "summary": "Stop agent" },
                { "method": "GET", "path": "/api/v1/agents/{id}/status", "summary": "Agent status" },
                { "method": "GET", "path": "/api/v1/agents/{id}/history", "summary": "Run history" },
                { "method": "POST", "path": "/api/v1/agents/{id}/autonomous", "summary": "Autonomous session" }
            ]
        },
        {
            "group": "Configuration",
            "items": [
                { "method": "GET", "path": "/api/v1/dashboard/config", "summary": "Aggregated config UI payload" },
                { "method": "GET", "path": "/api/v1/providers", "summary": "LLM providers" },
                { "method": "POST", "path": "/api/v1/providers", "summary": "Register provider" },
                { "method": "POST", "path": "/api/v1/providers/{id}/test", "summary": "Test provider" },
                { "method": "GET", "path": "/api/v1/channels", "summary": "Communication channels" },
                { "method": "GET", "path": "/api/v1/system/config", "summary": "System settings" },
                { "method": "PUT", "path": "/api/v1/system/config", "summary": "Update system settings" },
                { "method": "GET", "path": "/api/v1/system/auth/status", "summary": "Auth mode" }
            ]
        },
        {
            "group": "Tools",
            "items": [
                { "method": "GET", "path": "/api/v1/dashboard/tools", "summary": "Catalog + Docker + MCP UI payload" },
                { "method": "GET", "path": "/api/v1/tools", "summary": "Registered tools" },
                { "method": "POST", "path": "/api/v1/tools/{id}/execute", "summary": "Execute tool" },
                { "method": "GET", "path": "/api/v1/tools/marketplace", "summary": "Tool marketplace" }
            ]
        },
        {
            "group": "Fleet",
            "items": [
                { "method": "GET", "path": "/api/v1/fleet", "summary": "Fleet nodes" },
                { "method": "GET", "path": "/api/v1/fleet/deployments", "summary": "Agent deployments" },
                { "method": "GET", "path": "/api/v1/fleet/mesh", "summary": "Mesh topology" },
                { "method": "POST", "path": "/api/v1/fleet/deploy", "summary": "Deploy agent to node" },
                { "method": "GET", "path": "/api/v1/fleet/kanban", "summary": "Deployment board" }
            ]
        },
        {
            "group": "Governance",
            "items": [
                { "method": "GET", "path": "/api/v1/governance/policies", "summary": "Policies" },
                { "method": "GET", "path": "/api/v1/governance/audit", "summary": "Audit log" },
                { "method": "GET", "path": "/api/v1/governance/trust/{agent_id}", "summary": "Trust score" },
                { "method": "POST", "path": "/api/v1/governance/evaluate", "summary": "Evaluate action" },
                { "method": "GET", "path": "/api/v1/governance/proposals", "summary": "Approval proposals" },
                { "method": "GET", "path": "/api/v1/system/prism", "summary": "PRISM-G status" }
            ]
        },
        {
            "group": "Rooms & conversations",
            "items": [
                { "method": "GET", "path": "/api/v1/rooms", "summary": "Multi-agent rooms" },
                { "method": "GET", "path": "/api/v1/conversations", "summary": "1:1 conversations" },
                { "method": "POST", "path": "/api/v1/rooms/{id}/messages", "summary": "Send room message" }
            ]
        },
        {
            "group": "Cloud deploy",
            "items": [
                { "method": "GET", "path": "/api/v1/cloud/providers", "summary": "Cloud adapters" },
                { "method": "GET", "path": "/api/v1/cloud/deployments", "summary": "Cloud deployments" },
                { "method": "POST", "path": "/api/v1/cloud/deploy", "summary": "Deploy to cloud" }
            ]
        }
    ])
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/overview", get(dashboard_overview))
}

/// `GET /dashboard/overview` — health, resource counts, and API catalog for the UI.
pub async fn dashboard_overview(State(state): State<AppState>) -> Json<Value> {
    let agents = state.agents.read().await;
    let conversations = state.conversations.read().await;
    let rooms = state.rooms.read().await;
    let channels = state.channels.read().await;
    let providers = state.providers.read().await;
    let tools = state.tools.read().await;
    let policies = state.policies.read().await;
    let nodes = state.fleet_nodes.read().await;
    let deployments = state.deployments.read().await;
    let audit = state.audit_log.read().await;

    let uptime_secs = (Utc::now() - state.start_time).num_seconds().max(0) as u64;
    let running_agents = agents
        .iter()
        .filter(|a| matches!(a.status, AgentStatus::Running))
        .count();
    let online_nodes = nodes.iter().filter(|n| n.status == "online").count();
    let pending_proposals = state
        .approval_workflow
        .list_pending()
        .await
        .len();

    Json(json!({
        "health": {
            "status": "healthy",
            "uptime_secs": uptime_secs,
            "version": env!("CARGO_PKG_VERSION"),
            "auth_disabled": std::env::var("CLAWZ_DISABLE_AUTH").ok().as_deref() == Some("1"),
        },
        "counts": {
            "agents": agents.len(),
            "agents_running": running_agents,
            "conversations": conversations.len(),
            "rooms": rooms.len(),
            "channels": channels.len(),
            "providers": providers.len(),
            "tools": tools.len(),
            "policies": policies.len(),
            "fleet_nodes": nodes.len(),
            "fleet_nodes_online": online_nodes,
            "deployments": deployments.len(),
            "audit_entries": audit.len(),
            "pending_approvals": pending_proposals,
        },
        "links": {
            "prometheus_metrics": "/api/v1/system/metrics",
            "openapi": "/api/v1/system/openapi",
            "mcp": "/api/v1/mcp",
        },
        "api_catalog": api_catalog(),
        "generated_at": Utc::now().to_rfc3339(),
    }))
}
