//! Fleet and deployment REST API.
//!
//! The fleet is the set of worker nodes that run agents. This module handles:
//! - Node CRUD (`FleetNodeRecord`)
//! - Mesh topology visualization (`/fleet/mesh`)
//! - Agent-to-node deployment scheduling (`/fleet/deploy`)
//! - Fleet-wide Prometheus-style metrics (`/fleet/metrics`)
//! - Legacy kanban endpoints kept for UI compatibility
//!
//! # Cross-module interactions
//! - `fleet_deploy` validates that both the target agent and the target node exist
//!   before recording a `DeploymentRecord`.
//! - `fleet_metrics` aggregates data from `AppState.agents`, `AppState.fleet_nodes`,
//!   and `AppState.deployments` to produce a unified health snapshot.

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

// Dependency: crate root types shared across fleet, agents, and governance modules.
use crate::{AppState, DeploymentRecord, FleetNodeRecord, GatewayError};

/// Assemble the fleet sub-router.
///
/// Mounted by the gateway at `/api/v1/fleet`.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Fleet nodes CRUD
        .route("/", get(list_fleet).post(create_fleet_node))
        .route("/{id}", get(get_fleet_node).put(update_fleet_node).delete(delete_fleet_node))
        // Fleet-level topology and deployment endpoints
        .route("/mesh", get(fleet_mesh))
        .route("/deploy", post(fleet_deploy))
        .route("/metrics", get(fleet_metrics))
        // Per-node extra endpoints kept for backward compatibility
        .route("/{id}/deploy", post(deploy_to_node))
        .route("/{id}/logs", get(node_logs))
        // Kanban stubs retained for legacy UI compatibility
        .route("/kanban", get(kanban_board))
        .route("/kanban/{id}/move", post(kanban_move))
}

// ─── Body types ───────────────────────────────────────────────────────────────

/// Request body for `POST /fleet` — register a new worker node.
#[derive(Debug, Deserialize)]
pub struct CreateFleetNodeBody {
    /// Human-readable node name (required).
    pub name: Option<String>,
    /// Node role discriminator, e.g. "worker", "gpu", "edge".
    pub node_type: Option<String>,
    /// Network host or IP address. Defaults to "localhost".
    pub host: Option<String>,
    /// Service port. Defaults to 8080.
    pub port: Option<u16>,
}

/// Request body for `PUT /fleet/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateFleetNodeBody {
    /// New display name.
    pub name: Option<String>,
    /// New node role.
    pub node_type: Option<String>,
    /// New network host.
    pub host: Option<String>,
    /// New service port.
    pub port: Option<u16>,
    /// Free-form status string ("online", "offline", "degraded", …).
    pub status: Option<String>,
}

/// Request body for `POST /fleet/deploy` — schedule an agent on a node.
#[derive(Debug, Deserialize)]
pub struct FleetDeployBody {
    /// ID of the agent to deploy (required).
    pub agent_id: Option<String>,
    /// ID of the target fleet node (required).
    pub node_id: Option<String>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /fleet` — list all registered fleet nodes.
async fn list_fleet(State(state): State<AppState>) -> Json<Value> {
    let nodes = state.fleet_nodes.read().await;
    Json(json!({ "data": *nodes, "total": nodes.len() }))
}

/// `POST /fleet` — register a new worker node.
///
/// Defaults keep the node usable out-of-the-box: `node_type` → "worker",
/// `host` → "localhost", `port` → 8080, `status` → "online".
async fn create_fleet_node(
    State(state): State<AppState>,
    Json(body): Json<CreateFleetNodeBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let name = body.name.ok_or_else(|| {
        GatewayError::Unprocessable("field 'name' is required".to_string())
    })?;
    let host = body.host.unwrap_or_else(|| "localhost".to_string());
    let port = body.port.unwrap_or(8080);

    let now = Utc::now();
    let record = FleetNodeRecord {
        id: Uuid::new_v4().to_string(),
        name,
        node_type: body.node_type.unwrap_or_else(|| "worker".to_string()),
        host,
        port,
        status: "online".to_string(),
        agent_ids: Vec::new(),
        created_at: now,
        updated_at: now,
    };

    let mut nodes = state.fleet_nodes.write().await;
    nodes.push(record.clone());
    Ok((StatusCode::CREATED, Json(json!(record))))
}

/// `GET /fleet/{id}` — fetch a single node.
async fn get_fleet_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let nodes = state.fleet_nodes.read().await;
    let record = nodes
        .iter()
        .find(|n| n.id == id)
        .ok_or_else(|| GatewayError::not_found("FleetNode", &id))?;
    Ok(Json(json!(record)))
}

/// `PUT /fleet/{id}` — partial update of a node.
async fn update_fleet_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateFleetNodeBody>,
) -> Result<Json<Value>, GatewayError> {
    let mut nodes = state.fleet_nodes.write().await;
    let record = nodes
        .iter_mut()
        .find(|n| n.id == id)
        .ok_or_else(|| GatewayError::not_found("FleetNode", &id))?;

    if let Some(name) = body.name { record.name = name; }
    if let Some(nt) = body.node_type { record.node_type = nt; }
    if let Some(host) = body.host { record.host = host; }
    if let Some(port) = body.port { record.port = port; }
    if let Some(status) = body.status { record.status = status; }
    record.updated_at = Utc::now();

    Ok(Json(json!(record.clone())))
}

/// `DELETE /fleet/{id}` — remove a node from the fleet.
async fn delete_fleet_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mut nodes = state.fleet_nodes.write().await;
    let pos = nodes
        .iter()
        .position(|n| n.id == id)
        .ok_or_else(|| GatewayError::not_found("FleetNode", &id))?;
    nodes.remove(pos);
    Ok(StatusCode::NO_CONTENT)
}

// ─── Fleet-level endpoints ────────────────────────────────────────────────────

/// `GET /fleet/mesh` — compute the mesh topology of currently online nodes.
///
/// Builds a fully-connected graph among online nodes with synthetic latency
/// values so the orchestrator can visualize network health before real probes
/// are implemented.
async fn fleet_mesh(State(state): State<AppState>) -> Json<Value> {
    let nodes = state.fleet_nodes.read().await;

    // Build a mesh topology: each online node is connected to every other online node.
    // Synthetic latency increases with index sum so the graph looks realistic in demos.
    let online: Vec<Value> = nodes
        .iter()
        .filter(|n| n.status == "online")
        .map(|n| json!({
            "id": n.id,
            "name": n.name,
            "node_type": n.node_type,
            "host": n.host,
            "port": n.port,
            "status": n.status,
            "agent_count": n.agent_ids.len(),
        }))
        .collect();

    let mut connections: Vec<Value> = Vec::new();
    for i in 0..online.len() {
        for j in (i + 1)..online.len() {
            connections.push(json!({
                "from": online[i]["id"],
                "to": online[j]["id"],
                // Synthetic latency: base 5 ms + 3 ms per combined index.
                "latency_ms": 5 + (i + j) * 3,
                "bandwidth_mbps": 1000,
            }));
        }
    }

    Json(json!({
        "nodes": online,
        "connections": connections,
        "total_nodes": nodes.len(),
        "online_nodes": online.len(),
    }))
}

/// `POST /fleet/deploy` — schedule an agent onto a specific fleet node.
///
/// Validates both sides of the mapping (agent must exist, node must exist)
/// before creating a `DeploymentRecord` and attaching the agent ID to the
/// node's `agent_ids` list so the mesh view stays consistent.
async fn fleet_deploy(
    State(state): State<AppState>,
    Json(body): Json<FleetDeployBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let agent_id = body.agent_id.ok_or_else(|| {
        GatewayError::Unprocessable("field 'agent_id' is required".to_string())
    })?;
    let node_id = body.node_id.ok_or_else(|| {
        GatewayError::Unprocessable("field 'node_id' is required".to_string())
    })?;

    // Verify both agent and node exist before recording the deployment.
    // Dependency: reads AppState.agents from the agents domain.
    {
        let agents = state.agents.read().await;
        if !agents.iter().any(|a| a.id == agent_id) {
            return Err(GatewayError::not_found("Agent", &agent_id));
        }
    }
    {
        let nodes = state.fleet_nodes.read().await;
        if !nodes.iter().any(|n| n.id == node_id) {
            return Err(GatewayError::not_found("FleetNode", &node_id));
        }
    }

    let now = Utc::now();
    let deployment = DeploymentRecord {
        id: Uuid::new_v4().to_string(),
        agent_id: agent_id.clone(),
        node_id: node_id.clone(),
        status: "running".to_string(),
        created_at: now,
        updated_at: now,
    };

    // Append the agent to the node's local list so mesh queries can report
    // per-node agent counts without a secondary lookup.
    {
        let mut nodes = state.fleet_nodes.write().await;
        if let Some(node) = nodes.iter_mut().find(|n| n.id == node_id) {
            if !node.agent_ids.contains(&agent_id) {
                node.agent_ids.push(agent_id.clone());
            }
        }
    }

    state.deployments.write().await.push(deployment.clone());

    Ok((StatusCode::CREATED, Json(json!(deployment))))
}

/// `GET /fleet/metrics` — aggregate fleet-wide statistics.
///
/// Pulls counters from agents, fleet nodes, and deployments to produce a
/// single JSON health snapshot for dashboard consumption.
async fn fleet_metrics(State(state): State<AppState>) -> Json<Value> {
    let nodes = state.fleet_nodes.read().await;
    let deployments = state.deployments.read().await;
    let agents = state.agents.read().await;

    let online_count = nodes.iter().filter(|n| n.status == "online").count();
    let offline_count = nodes.iter().filter(|n| n.status == "offline").count();
    let degraded_count = nodes.iter().filter(|n| n.status == "degraded").count();
    let active_deployments = deployments.iter().filter(|d| d.status == "running").count();
    let running_agents = agents.iter().filter(|a| {
        matches!(a.status, crate::AgentStatus::Running)
    }).count();

    Json(json!({
        "nodes": {
            "total": nodes.len(),
            "online": online_count,
            "offline": offline_count,
            "degraded": degraded_count,
        },
        "deployments": {
            "total": deployments.len(),
            "active": active_deployments,
        },
        "agents": {
            "total": agents.len(),
            "running": running_agents,
        },
        "computed_at": Utc::now(),
    }))
}

// ─── Per-node extra endpoints ─────────────────────────────────────────────────

/// `POST /fleet/{id}/deploy` — legacy per-node deploy stub.
async fn deploy_to_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let nodes = state.fleet_nodes.read().await;
    nodes
        .iter()
        .find(|n| n.id == id)
        .ok_or_else(|| GatewayError::not_found("FleetNode", &id))?;
    Ok(Json(json!({ "node_id": id, "status": "deploying", "deployment_id": Uuid::new_v4().to_string() })))
}

/// `GET /fleet/{id}/logs` — synthetic node log stream.
async fn node_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let nodes = state.fleet_nodes.read().await;
    nodes
        .iter()
        .find(|n| n.id == id)
        .ok_or_else(|| GatewayError::not_found("FleetNode", &id))?;
    Ok(Json(json!({
        "node_id": id,
        "logs": [
            { "level": "info", "message": "Node started", "timestamp": Utc::now() },
            { "level": "info", "message": "Agent deployed", "timestamp": Utc::now() },
        ],
        "total": 2
    })))
}

// ─── Kanban (legacy compat) ───────────────────────────────────────────────────

/// `GET /fleet/kanban` — legacy kanban board stub.
async fn kanban_board() -> Json<Value> {
    Json(json!({
        "columns": [
            { "id": "col-1", "name": "Backlog", "tasks": [] },
            { "id": "col-2", "name": "In Progress", "tasks": [] },
            { "id": "col-3", "name": "Done", "tasks": [] },
        ]
    }))
}

/// `POST /fleet/kanban/{id}/move` — legacy kanban move stub.
async fn kanban_move(Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "id": id, "moved": true }))
}
