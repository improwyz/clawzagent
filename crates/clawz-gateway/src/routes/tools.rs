//! Tool registry and execution REST API.
//!
//! Tools are capabilities that agents can invoke — function calls, API wrappers,
//! skills, and plugins. Each `ToolRecord` carries an opaque JSON `config` so the
//! schema can evolve without migration. This module covers:
//! - Tool CRUD
//! - Simulated execution (`POST /tools/{id}/execute`)
//! - Filtered listings for skills and plugins
//! - A hard-coded marketplace catalog for discoverability
//!
//! # Cross-module interactions
//! - `execute_tool` reads `AppState.tools` and simulates a result; in production
//!   this will delegate to the actual tool runtime (e.g. MCP server, WASM plugin).

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

// Dependency: AppState, GatewayError, ToolRecord defined in crate root.
use crate::{AppState, GatewayError, ToolRecord};

/// Assemble the tool sub-router.
///
/// Mounted by the gateway at `/api/v1/tools`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_tools).post(create_tool))
        .route("/{id}", get(get_tool).put(update_tool).delete(delete_tool))
        .route("/{id}/execute", post(execute_tool))
        // Additional informational / discoverability endpoints
        .route("/skills", get(list_skills))
        .route("/plugins", get(list_plugins))
        .route("/marketplace", get(list_marketplace))
}

// ─── Body types ───────────────────────────────────────────────────────────────

/// Request body for `POST /tools`.
#[derive(Debug, Deserialize)]
pub struct CreateToolBody {
    /// Human-readable tool name (required).
    pub name: Option<String>,
    /// Short explanation shown in UI listings and agent prompts.
    pub description: Option<String>,
    /// Kind discriminator: "function", "api", "skill", "plugin", "mcp", etc.
    pub tool_type: Option<String>,
    /// Opaque JSON configuration consumed by the tool adapter / runtime.
    pub config: Option<serde_json::Value>,
    /// Whether the tool is available for agent invocation. Defaults to `true`.
    pub enabled: Option<bool>,
}

/// Request body for `PUT /tools/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateToolBody {
    /// New display name.
    pub name: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New kind discriminator.
    pub tool_type: Option<String>,
    /// Replaced configuration blob.
    pub config: Option<serde_json::Value>,
    /// Enable / disable toggle.
    pub enabled: Option<bool>,
}

/// Request body for `POST /tools/{id}/execute`.
#[derive(Debug, Deserialize)]
pub struct ExecuteToolBody {
    /// Positional or named arguments forwarded to the tool runtime.
    pub args: Option<serde_json::Value>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /tools` — list every registered tool.
async fn list_tools(State(state): State<AppState>) -> Json<Value> {
    let tools = state.tools.read().await;
    Json(json!({ "data": *tools, "total": tools.len() }))
}

/// `POST /tools` — register a new tool.
///
/// Defaults: `tool_type` → "function", `config` → `{}`, `enabled` → `true`.
async fn create_tool(
    State(state): State<AppState>,
    Json(body): Json<CreateToolBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let name = body.name.ok_or_else(|| {
        GatewayError::Unprocessable("field 'name' is required".to_string())
    })?;
    let description = body.description.unwrap_or_default();
    let tool_type = body.tool_type.unwrap_or_else(|| "function".to_string());

    let now = Utc::now();
    let record = ToolRecord {
        id: Uuid::new_v4().to_string(),
        name,
        description,
        tool_type,
        config: body.config.unwrap_or(json!({})),
        enabled: body.enabled.unwrap_or(true),
        created_at: now,
        updated_at: now,
    };

    let mut tools = state.tools.write().await;
    tools.push(record.clone());
    Ok((StatusCode::CREATED, Json(json!(record))))
}

/// `GET /tools/{id}` — fetch a single tool.
async fn get_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let tools = state.tools.read().await;
    let record = tools
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| GatewayError::not_found("Tool", &id))?;
    Ok(Json(json!(record)))
}

/// `PUT /tools/{id}` — partial update.
async fn update_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateToolBody>,
) -> Result<Json<Value>, GatewayError> {
    let mut tools = state.tools.write().await;
    let record = tools
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| GatewayError::not_found("Tool", &id))?;

    if let Some(name) = body.name { record.name = name; }
    if let Some(desc) = body.description { record.description = desc; }
    if let Some(tt) = body.tool_type { record.tool_type = tt; }
    if let Some(cfg) = body.config { record.config = cfg; }
    if let Some(enabled) = body.enabled { record.enabled = enabled; }
    record.updated_at = Utc::now();

    Ok(Json(json!(record.clone())))
}

/// `DELETE /tools/{id}` — unregister a tool.
async fn delete_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mut tools = state.tools.write().await;
    let pos = tools
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| GatewayError::not_found("Tool", &id))?;
    tools.remove(pos);
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /tools/{id}/execute` — simulate a tool invocation.
///
/// Rejects execution if the tool is disabled so agents cannot accidentally
/// invoke an off-line capability. In production this would route to the
/// appropriate runtime (Docker container, WASM sandbox, MCP server, etc.).
async fn execute_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ExecuteToolBody>,
) -> Result<Json<Value>, GatewayError> {
    let tools = state.tools.read().await;
    let record = tools
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| GatewayError::not_found("Tool", &id))?;

    if !record.enabled {
        return Err(GatewayError::Unprocessable(format!("Tool '{}' is disabled", record.name)));
    }

    let args = body.args.unwrap_or(json!({}));

    // Simulate tool execution — returns a deterministic placeholder so UI
    // tests and demos have a stable shape to assert against.
    Ok(Json(json!({
        "tool_id": id,
        "tool_name": record.name,
        "tool_type": record.tool_type,
        "args": args,
        "result": {
            "status": "success",
            "output": format!("Tool '{}' executed successfully", record.name),
            "execution_time_ms": 23,
        },
        "executed_at": Utc::now(),
    })))
}

// ─── Informational endpoints ──────────────────────────────────────────────────

/// `GET /tools/skills` — subset of tools whose `tool_type` is "skill".
async fn list_skills(State(state): State<AppState>) -> Json<Value> {
    let tools = state.tools.read().await;
    let skills: Vec<&ToolRecord> = tools.iter().filter(|t| t.tool_type == "skill").collect();
    Json(json!({ "data": skills, "total": skills.len() }))
}

/// `GET /tools/plugins` — subset of tools whose `tool_type` is "plugin".
async fn list_plugins(State(state): State<AppState>) -> Json<Value> {
    let tools = state.tools.read().await;
    let plugins: Vec<&ToolRecord> = tools.iter().filter(|t| t.tool_type == "plugin").collect();
    Json(json!({ "data": plugins, "total": plugins.len() }))
}

/// `GET /tools/marketplace` — hard-coded catalog of installable tools.
///
/// Serves as a placeholder until a real marketplace backend (with versioning,
/// publisher verification, and search) is implemented.
async fn list_marketplace(_state: State<AppState>) -> Json<Value> {
    // Placeholder marketplace catalog — in production this would query a
    // registry service or package index.
    Json(json!({
        "data": [
            { "id": "mkt-001", "name": "Web Search", "tool_type": "api", "description": "Search the web", "publisher": "clawz", "version": "1.0.0" },
            { "id": "mkt-002", "name": "Code Interpreter", "tool_type": "function", "description": "Execute code snippets", "publisher": "clawz", "version": "1.2.0" },
            { "id": "mkt-003", "name": "Database Query", "tool_type": "mcp", "description": "Run SQL against databases", "publisher": "clawz", "version": "0.9.1" },
        ],
        "total": 3
    }))
}
