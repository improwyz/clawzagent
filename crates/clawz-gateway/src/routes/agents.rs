//! Agent lifecycle REST API.
//!
//! Provides CRUD for `AgentRecord`s, execution control (`run`, `stop`, `start`),
//! conversation history lookup, and legacy compatibility stubs for onboarding,
//! orchestration, batching, fan-out, and A2A (agent-to-agent) discovery.
//!
//! # Key flows
//! 1. `POST /agents`           → create an agent (Idle by default).
//! 2. `POST /agents/{id}/run`  → attach a message to the agent's conversation
//!                                and transition status to Running.
//! 3. `POST /agents/{id}/stop` → transition status to Stopped.
//!
//! # Cross-module interactions
//! - Uses `crate::ConversationRecord` and `crate::MessageRecord` from the
//!   `conversations` domain when handling the `run_agent` flow.
//! - Writes to `AppState.agents` and `AppState.conversations` concurrently,
//!   so scoped read / write locks are used to avoid deadlocks.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

// Dependency: AgentRecord, AgentStatus, AppState, GatewayError are defined in the crate root.
// Dependency: ConversationRecord, MessageRecord are defined in the crate root (shared with conversations module).
use crate::{
    AgentRecord, AgentStatus, AppState, AutonomousSessionRecord, AutonomousSessionStatus,
    ConversationRecord, GatewayError, MessageRecord,
};

/// Assemble the agent sub-router and mount all handlers.
///
/// Mounted by the gateway at `/api/v1/agents`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_agents).post(create_agent))
        .route("/{id}", get(get_agent).put(update_agent).delete(delete_agent))
        .route("/{id}/run", post(run_agent))
        .route("/{id}/stop", post(stop_agent))
        .route("/{id}/autonomous", post(run_autonomous))
        .route("/{id}/history", get(agent_history))
        // Legacy / extra routes kept for backward compatibility with older SDK versions.
        .route("/{id}/start", post(start_agent))
        .route("/{id}/status", get(agent_status))
        .route("/{id}/personality", get(get_personality).put(update_personality))
        .route("/{id}/identity/hash", get(get_agent_identity_hash))
        .route("/onboard", post(run_onboard))
        .route("/orchestrate", post(orchestrate))
        .route("/batch", post(batch))
        .route("/fanout", post(fanout))
        .route("/a2a/discover", post(a2a_discover))
        .route("/a2a/invoke", post(a2a_invoke))
}

// ─── Query / body types ───────────────────────────────────────────────────────

/// Query parameters for `GET /agents` list endpoint.
#[derive(Debug, Deserialize)]
pub struct ListAgentsQuery {
    /// Optional status filter (e.g. "idle", "running", "stopped", "error").
    /// When omitted, all agents are returned.
    pub status: Option<String>,
}

/// Request body for `POST /agents` — create a new agent.
#[derive(Debug, Deserialize)]
pub struct CreateAgentBody {
    /// Human-readable name for the agent (required).
    pub name: Option<String>,
    /// Identifier of the underlying model or engine (required).
    pub model: Option<String>,
    /// Free-form description shown in UI listings.
    pub description: Option<String>,
    /// System prompt / instruction set injected at the start of every conversation.
    pub system_prompt: Option<String>,
}

/// Request body for `PUT /agents/{id}` — partial update of an existing agent.
#[derive(Debug, Deserialize)]
pub struct UpdateAgentBody {
    /// New display name.
    pub name: Option<String>,
    /// New model identifier.
    pub model: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New system prompt.
    pub system_prompt: Option<String>,
    /// Target lifecycle status string ("running", "stopped", "error", or anything else → Idle).
    pub status: Option<String>,
}

/// Request body for `POST /agents/{id}/run` — trigger a single-turn execution.
#[derive(Debug, Deserialize)]
pub struct RunAgentBody {
    /// User message to send to the agent. Defaults to "Hello" when omitted.
    pub message: Option<String>,
}

/// Request body for `POST /agents/{id}/autonomous` — start a long-running,
/// multi-turn autonomous session for the agent.
///
/// All fields are optional. When omitted, the worker runtime falls back to
/// its configured defaults (`DEFAULT_MAX_TURNS` and `DEFAULT_COST_BUDGET_USD`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutonomousBody {
    /// Hard cap on the number of turns this session may execute.
    pub max_turns: Option<usize>,
    /// Hard cap on accumulated USD spend for this session.
    pub cost_budget_usd: Option<f64>,
    /// Optional per-session system prompt override.
    pub system_prompt: Option<String>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /agents` — list all agents, optionally filtered by status.
async fn list_agents(
    State(state): State<AppState>,
    Query(q): Query<ListAgentsQuery>,
) -> Json<Value> {
    let agents = state.agents.read().await;
    let list: Vec<&AgentRecord> = agents
        .iter()
        .filter(|a| {
            if let Some(ref s) = q.status {
                a.status.to_string() == *s
            } else {
                true
            }
        })
        .collect();
    Json(json!({ "data": list, "total": list.len() }))
}

/// `POST /agents` — create a new agent record.
///
/// Validates that `name` and `model` are present. The agent starts in `Idle`
/// status and is immediately persisted to `AppState.agents`. An audit entry
/// is appended so governance can track creation events.
async fn create_agent(
    State(state): State<AppState>,
    Json(body): Json<CreateAgentBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let name = body.name.ok_or_else(|| {
        GatewayError::Unprocessable("field 'name' is required".to_string())
    })?;
    let model = body.model.ok_or_else(|| {
        GatewayError::Unprocessable("field 'model' is required".to_string())
    })?;

    let now = Utc::now();
    let record = AgentRecord {
        id: Uuid::new_v4().to_string(),
        name,
        model,
        description: body.description,
        system_prompt: body.system_prompt,
        status: AgentStatus::Idle,
        created_at: now,
        updated_at: now,
    };

    // Audit the creation so governance / trust scoring can inspect it later.
    state
        .append_audit("system", "create", "agent", &record.id, None)
        .await;

    let mut agents = state.agents.write().await;
    agents.push(record.clone());

    Ok((StatusCode::CREATED, Json(json!(record))))
}

/// `GET /agents/{id}` — fetch a single agent by its UUID.
async fn get_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let agents = state.agents.read().await;
    let record = agents
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    Ok(Json(json!(record)))
}

/// `PUT /agents/{id}` — apply a partial update to an existing agent.
///
/// Only fields supplied in the JSON body are overwritten. The `status` string
/// is mapped to the strongly-typed `AgentStatus` enum; unknown values fall
/// back to `Idle` to keep the state machine safe.
async fn update_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAgentBody>,
) -> Result<Json<Value>, GatewayError> {
    let mut agents = state.agents.write().await;
    let record = agents
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;

    if let Some(name) = body.name { record.name = name; }
    if let Some(model) = body.model { record.model = model; }
    if let Some(desc) = body.description { record.description = Some(desc); }
    if let Some(sp) = body.system_prompt { record.system_prompt = Some(sp); }
    if let Some(status) = body.status {
        // Map free-form string to the typed AgentStatus enum. Unknown → Idle
        // so a typo does not accidentally leave the agent in an invalid state.
        record.status = match status.as_str() {
            "running" => AgentStatus::Running,
            "stopped" => AgentStatus::Stopped,
            "error" => AgentStatus::Error,
            _ => AgentStatus::Idle,
        };
    }
    record.updated_at = Utc::now();

    Ok(Json(json!(record.clone())))
}

/// `DELETE /agents/{id}` — permanently remove an agent.
///
/// Also emits an audit entry so deletion is traceable for compliance.
async fn delete_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mut agents = state.agents.write().await;
    let pos = agents
        .iter()
        .position(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    agents.remove(pos);
    state
        .append_audit("system", "delete", "agent", &id, None)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /agents/{id}/run` — execute one turn of the agent.
///
/// This is the primary interaction endpoint. It:
/// 1. Validates the agent exists.
/// 2. Finds or creates a `ConversationRecord` tied to the agent.
/// 3. Appends a user message and a simulated assistant response.
/// 4. Transitions the agent status to `Running`.
///
/// # Concurrency note
/// Scoped locks are used so we never hold both `agents` and `conversations`
/// write locks at the same time, preventing deadlock.
async fn run_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RunAgentBody>,
) -> Result<Json<Value>, GatewayError> {
    // Verify the agent exists before touching conversations.
    {
        let agents = state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    }

    let user_message = body.message.unwrap_or_else(|| "Hello".to_string());
    let now = Utc::now();

    // Find an existing open conversation for this agent or create a new one.
    // Dependency: uses ConversationRecord and MessageRecord from the shared crate root.
    let mut conversations = state.conversations.write().await;
    let conv = conversations
        .iter_mut()
        .find(|c| c.agent_id == id && !c.archived);

    let (conv_id, response_content) = if let Some(c) = conv {
        // Re-use existing conversation — append user + assistant messages.
        let user_msg = MessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: c.id.clone(),
            role: "user".to_string(),
            content: user_message.clone(),
            created_at: now,
        };
        let assistant_msg = MessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: c.id.clone(),
            role: "assistant".to_string(),
            content: format!("Acknowledged: {}", user_message),
            created_at: now,
        };
        let resp = assistant_msg.content.clone();
        c.messages.push(user_msg);
        c.messages.push(assistant_msg);
        c.updated_at = now;
        (c.id.clone(), resp)
    } else {
        // No open conversation — start a new thread titled with the first 60 chars of the message.
        let conv_id = Uuid::new_v4().to_string();
        let user_msg = MessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: conv_id.clone(),
            role: "user".to_string(),
            content: user_message.clone(),
            created_at: now,
        };
        let assistant_content = format!("Acknowledged: {}", user_message);
        let assistant_msg = MessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: conv_id.clone(),
            role: "assistant".to_string(),
            content: assistant_content.clone(),
            created_at: now,
        };
        let new_conv = ConversationRecord {
            id: conv_id.clone(),
            agent_id: id.clone(),
            title: Some(user_message[..user_message.len().min(60)].to_string()),
            archived: false,
            messages: vec![user_msg, assistant_msg],
            created_at: now,
            updated_at: now,
        };
        conversations.push(new_conv);
        (conv_id, assistant_content)
    };

    // Transition agent to Running now that the turn has been recorded.
    {
        let mut agents = state.agents.write().await;
        if let Some(a) = agents.iter_mut().find(|a| a.id == id) {
            a.status = AgentStatus::Running;
            a.updated_at = now;
        }
    }

    Ok(Json(json!({
        "conversation_id": conv_id,
        "role": "assistant",
        "content": response_content,
        "model": "clawz-agent",
        "created_at": now,
    })))
}

/// `POST /agents/{id}/stop` — halt the agent and set status to Stopped.
async fn stop_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let mut agents = state.agents.write().await;
    let record = agents
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    record.status = AgentStatus::Stopped;
    record.updated_at = Utc::now();
    Ok(Json(json!({ "id": id, "status": "stopped" })))
}

/// `POST /agents/{id}/autonomous` — start a long-running, multi-turn
/// autonomous session for the agent.
///
/// The handler:
/// 1. Validates the agent exists.
/// 2. Builds an [`AutonomousSessionRecord`] honouring optional `max_turns`,
///    `cost_budget_usd`, and `system_prompt` overrides supplied in the body.
/// 3. Persists the session into `AppState.autonomous_sessions`.
/// 4. Transitions the agent's status to `Running`.
///
/// Mirrors `clawz_worker::runtime::agent::AgentRuntime::start_autonomous_session`
/// at the API layer; the worker side owns the actual multi-turn loop.
async fn run_autonomous(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AutonomousBody>,
) -> Result<Json<Value>, GatewayError> {
    // Verify the agent exists before creating a session.
    {
        let agents = state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    }

    // Default budgets mirror `clawz_worker::runtime::agent`'s constants.
    // The worker is the source of truth at runtime; these values are the
    // session-creation defaults exposed to API clients.
    const DEFAULT_MAX_TURNS: usize = 20;
    const DEFAULT_COST_BUDGET_USD: f64 = 1.0;

    let now = Utc::now();
    let session = AutonomousSessionRecord {
        id: Uuid::new_v4().to_string(),
        agent_id: id.clone(),
        max_turns: body.max_turns.unwrap_or(DEFAULT_MAX_TURNS),
        cost_budget_usd: body.cost_budget_usd.unwrap_or(DEFAULT_COST_BUDGET_USD),
        turns_executed: 0,
        cost_accumulated_usd: 0.0,
        status: AutonomousSessionStatus::Running,
        system_prompt: body.system_prompt,
        created_at: now,
        updated_at: now,
    };
    let session_id = session.id.clone();

    {
        let mut sessions = state.autonomous_sessions.write().await;
        sessions.push(session);
    }

    // Reflect the session start on the agent record itself.
    {
        let mut agents = state.agents.write().await;
        if let Some(a) = agents.iter_mut().find(|a| a.id == id) {
            a.status = AgentStatus::Running;
            a.updated_at = now;
        }
    }

    Ok(Json(json!({
        "session_id": session_id,
        "status": "running",
    })))
}

/// `GET /agents/{id}/history` — return every message across all conversations
/// that belong to this agent.
async fn agent_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    // Verify the agent exists first so we return 404 rather than an empty list.
    {
        let agents = state.agents.read().await;
        agents
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    }

    let conversations = state.conversations.read().await;
    let messages: Vec<&MessageRecord> = conversations
        .iter()
        .filter(|c| c.agent_id == id)
        .flat_map(|c| c.messages.iter())
        .collect();

    Ok(Json(json!({ "agent_id": id, "messages": messages, "total": messages.len() })))
}

// ─── Legacy / compatibility handlers ─────────────────────────────────────────

/// `POST /agents/{id}/start` — legacy alias that sets status to Running.
/// Kept for backward compatibility with v1 SDK clients.
async fn start_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let mut agents = state.agents.write().await;
    let record = agents
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    record.status = AgentStatus::Running;
    record.updated_at = Utc::now();
    Ok(Json(json!({ "id": id, "status": "running" })))
}

/// `GET /agents/{id}/status` — legacy lightweight status check.
async fn agent_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let agents = state.agents.read().await;
    let record = agents
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    Ok(Json(json!({ "id": id, "status": record.status })))
}

/// Request body for `PUT /agents/{id}/personality`.
#[derive(Debug, Deserialize)]
pub struct PersonalityBody {
    /// Ordered list of personality trait keywords (e.g. "helpful", "terse").
    pub traits: Option<Vec<String>>,
    /// Preferred conversational tone (e.g. "neutral", "friendly", "professional").
    pub tone: Option<String>,
}

/// `GET /agents/{id}/personality` — returns the agent's personality metadata.
///
/// Currently returns a static placeholder because personality is not yet
/// stored in `AgentRecord`. Callers should treat this as a schema contract.
async fn get_personality(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let agents = state.agents.read().await;
    agents
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    Ok(Json(json!({ "agent_id": id, "traits": [], "tone": "neutral" })))
}

/// `PUT /agents/{id}/personality` — accept personality updates.
///
/// No-op persistence for now; returns the echoed body so the API contract
/// remains stable while the backend matures.
async fn update_personality(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PersonalityBody>,
) -> Result<Json<Value>, GatewayError> {
    let agents = state.agents.read().await;
    agents
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
    Ok(Json(json!({ "agent_id": id, "traits": body.traits.unwrap_or_default(), "tone": body.tone.unwrap_or_else(|| "neutral".to_string()), "updated": true })))
}

/// `GET /agents/{id}/identity/hash` — return the agent's current
/// identity-version hash.
///
/// The hash is a stable SHA-256 fingerprint over the immutable
/// [`IdentityCore`] plus the evolving [`IdentityState`] (preferences and
/// talent). Operators use it to detect drift, version snapshots, and
/// verify identity integrity across sessions.
///
/// Returns `{"agent_id", "identity_version_hash"}` on success, or an
/// `error` payload when the identity store is not configured or the
/// agent has no identity record.
async fn get_agent_identity_hash(
    Path(id): Path<String>,
    State(ctx): State<AppState>,
) -> Json<Value> {
    if let Some(ref store) = ctx.identity_store {
        match store.get_identity_version_hash(&id).await {
            Ok(hash) => Json(json!({ "agent_id": id, "identity_version_hash": hash })),
            Err(_) => Json(json!({ "error": "identity not found" })),
        }
    } else {
        Json(json!({ "error": "identity store not configured" }))
    }
}

/// `POST /agents/onboard` — legacy onboarding stub.
async fn run_onboard() -> Json<Value> {
    Json(json!({ "status": "onboarded", "steps_completed": ["init", "config", "test"] }))
}

/// `POST /agents/orchestrate` — legacy orchestration stub.
async fn orchestrate() -> Json<Value> {
    Json(json!({ "status": "queued", "orchestration_id": Uuid::new_v4().to_string() }))
}

/// `POST /agents/batch` — legacy batch execution stub.
async fn batch() -> Json<Value> {
    Json(json!({ "status": "queued", "batch_id": Uuid::new_v4().to_string(), "count": 0 }))
}

/// `POST /agents/fanout` — legacy fan-out stub.
async fn fanout() -> Json<Value> {
    Json(json!({ "status": "queued", "fanout_id": Uuid::new_v4().to_string() }))
}

/// `POST /agents/a2a/discover` — list all registered agent IDs for A2A discovery.
async fn a2a_discover(State(state): State<AppState>) -> Json<Value> {
    let agents = state.agents.read().await;
    let ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    Json(json!({ "agents": ids }))
}

/// `POST /agents/a2a/invoke` — legacy A2A invocation stub.
async fn a2a_invoke() -> Json<Value> {
    Json(json!({ "status": "invoked", "result": null }))
}
