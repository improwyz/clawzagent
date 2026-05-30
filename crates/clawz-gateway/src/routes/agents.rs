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
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use clawz_services::dto::{A2aInvokeRequest, FanOutRequest, OrchestrateRequest, RunTurnRequest};

// Dependency: AgentRecord, AgentStatus, AppState, GatewayError are defined in the crate root.
// Dependency: ConversationRecord, MessageRecord are defined in the crate root (shared with conversations module).
use crate::{
    AgentRecord, AgentStatus, AppState, AutonomousSessionRecord, AutonomousSessionStatus,
    ChannelRecord, ConversationRecord, GatewayError, MessageRecord,
};

/// Assemble the agent sub-router and mount all handlers.
///
/// Mounted by the gateway at `/api/v1/agents`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_agents).post(create_agent))
        .route(
            "/{id}",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
        .route("/{id}/run", post(run_agent))
        .route(
            "/{id}/runs/{run_id}/events",
            get(crate::routes::run_events::run_events_sse),
        )
        .route("/{id}/stop", post(stop_agent))
        .route("/{id}/autonomous", post(run_autonomous))
        .route("/{id}/history", get(agent_history))
        // Legacy / extra routes kept for backward compatibility with older SDK versions.
        .route("/{id}/start", post(start_agent))
        .route("/{id}/status", get(agent_status))
        .route(
            "/{id}/personality",
            get(get_personality).put(update_personality),
        )
        .route("/{id}/identity/hash", get(get_agent_identity_hash))
        .route("/onboard", post(run_onboard))
        .route("/orchestrate", post(orchestrate))
        .route("/batch", post(batch))
        .route("/fanout", post(fanout))
        .route("/a2a/discover", post(a2a_discover))
        .route("/a2a/invoke", post(a2a_invoke))
        .route("/{id}/phone", post(bind_agent_phone))
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
    /// First user message for the autonomous session (defaults to a generic start).
    pub initial_message: Option<String>,
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
    let name = body
        .name
        .ok_or_else(|| GatewayError::Unprocessable("field 'name' is required".to_string()))?;
    let model = body
        .model
        .ok_or_else(|| GatewayError::Unprocessable("field 'model' is required".to_string()))?;

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

    state.persist_agent_record(&record).await;

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

    if let Some(name) = body.name {
        record.name = name;
    }
    if let Some(model) = body.model {
        record.model = model;
    }
    if let Some(desc) = body.description {
        record.description = Some(desc);
    }
    if let Some(sp) = body.system_prompt {
        record.system_prompt = Some(sp);
    }
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

/// `POST /agents/{id}/run` — execute one turn via the worker pipeline.
async fn run_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RunAgentBody>,
) -> Result<Json<Value>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let (model, system_prompt) = {
        let agents = state.agents.read().await;
        let agent = agents
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| GatewayError::not_found("Agent", &id))?;
        (agent.model.clone(), agent.system_prompt.clone())
    };

    let user_message = body.message.unwrap_or_else(|| "Hello".to_string());
    let now = Utc::now();

    let existing_conv_id = {
        let conversations = state.conversations.read().await;
        conversations
            .iter()
            .find(|c| c.agent_id == id && !c.archived)
            .map(|c| c.id.clone())
    };

    let turn = platform
        .execution
        .run_turn(
            &id,
            RunTurnRequest {
                message: user_message.clone(),
                model: Some(model.clone()),
                system_prompt: system_prompt.clone(),
                conversation_id: existing_conv_id.clone(),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    let response_content = turn.content;
    let conv_id = turn.conversation_id;

    let mut conversations = state.conversations.write().await;
    if let Some(c) = conversations.iter_mut().find(|c| c.id == conv_id) {
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
            content: response_content.clone(),
            created_at: now,
        };
        c.messages.push(user_msg);
        c.messages.push(assistant_msg);
        c.updated_at = now;
    } else {
        let user_msg = MessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: conv_id.clone(),
            role: "user".to_string(),
            content: user_message.clone(),
            created_at: now,
        };
        let assistant_msg = MessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: conv_id.clone(),
            role: "assistant".to_string(),
            content: response_content.clone(),
            created_at: now,
        };
        conversations.push(ConversationRecord {
            id: conv_id.clone(),
            agent_id: id.clone(),
            title: Some(user_message[..user_message.len().min(60)].to_string()),
            archived: false,
            messages: vec![user_msg, assistant_msg],
            created_at: now,
            updated_at: now,
        });
    }

    {
        let mut agents = state.agents.write().await;
        if let Some(a) = agents.iter_mut().find(|a| a.id == id) {
            a.status = AgentStatus::Running;
            a.updated_at = now;
        }
    }

    platform.events.publish_json(
        "agent.run",
        json!({
            "agent_id": id,
            "conversation_id": conv_id,
        }),
    );

    if let Some(ref pool) = state.db {
        crate::postgres_store::persist_messages(pool, &conv_id, &user_message, &response_content)
            .await;
    }

    Ok(Json(json!({
        "conversation_id": conv_id,
        "role": turn.role,
        "content": response_content,
        "model": model,
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
    let conversation_id = format!("autonomous-{session_id}");
    let max_turns = session.max_turns;
    let system_prompt = session.system_prompt.clone();

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

    state.publish_event(
        "agent.autonomous.start",
        json!({
            "session_id": session_id,
            "conversation_id": conversation_id,
            "agent_id": id,
        }),
    );

    if let Some(platform) = state.platform.clone() {
        let state_bg = state.clone();
        let agent_id = id.clone();
        let sid = session_id.clone();
        let conv_id = conversation_id.clone();
        let model = {
            let agents = state.agents.read().await;
            agents.iter().find(|a| a.id == id).map(|a| a.model.clone())
        };
        let initial_prompt = body
            .initial_message
            .clone()
            .unwrap_or_else(|| "Begin your autonomous task.".to_string());
        tokio::spawn(async move {
            let mut turn = 0usize;
            let mut last_content = String::new();
            while turn < max_turns {
                {
                    let sessions = state_bg.autonomous_sessions.read().await;
                    if let Some(s) = sessions.iter().find(|s| s.id == sid) {
                        if s.status != AutonomousSessionStatus::Running {
                            break;
                        }
                    }
                }

                let msg = if turn == 0 {
                    initial_prompt.clone()
                } else {
                    format!("Continue your task. Previous output:\n{last_content}")
                };

                match platform
                    .execution
                    .run_turn(
                        &agent_id,
                        RunTurnRequest {
                            message: msg,
                            model: model.clone(),
                            system_prompt: system_prompt.clone(),
                            conversation_id: Some(conv_id.clone()),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    Ok(resp) => {
                        last_content = resp.content.clone();
                        state_bg.publish_event(
                            "agent.autonomous.turn",
                            json!({
                                "session_id": sid,
                                "conversation_id": conv_id,
                                "agent_id": agent_id,
                                "turn": turn,
                                "run_id": resp.run_id,
                                "content": resp.content,
                            }),
                        );
                        let mut sessions = state_bg.autonomous_sessions.write().await;
                        if let Some(s) = sessions.iter_mut().find(|s| s.id == sid) {
                            s.turns_executed = turn + 1;
                            s.updated_at = Utc::now();
                        }
                        turn += 1;
                    }
                    Err(e) => {
                        state_bg.publish_event(
                            "agent.autonomous.error",
                            json!({ "session_id": sid, "error": e.to_string() }),
                        );
                        break;
                    }
                }
            }

            let mut sessions = state_bg.autonomous_sessions.write().await;
            if let Some(s) = sessions.iter_mut().find(|s| s.id == sid) {
                s.status = AutonomousSessionStatus::Completed;
                s.updated_at = Utc::now();
            }
            state_bg.publish_event(
                "agent.autonomous.end",
                json!({
                    "session_id": sid,
                    "conversation_id": conv_id,
                    "agent_id": agent_id,
                    "total_turns": turn,
                }),
            );
        });
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

    Ok(Json(
        json!({ "agent_id": id, "messages": messages, "total": messages.len() }),
    ))
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

    let personality = state.personality.read().await;
    if let Some(prefs) = personality.get(&id) {
        return Ok(Json(prefs.clone()));
    }

    Ok(Json(
        json!({ "agent_id": id, "traits": [], "tone": "neutral" }),
    ))
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

    let prefs = json!({
        "agent_id": id,
        "traits": body.traits.unwrap_or_default(),
        "tone": body.tone.unwrap_or_else(|| "neutral".to_string()),
        "updated": true,
    });
    state
        .personality
        .write()
        .await
        .insert(id.clone(), prefs.clone());

    Ok(Json(prefs))
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

/// `POST /agents/onboard` — parse a goal, create an agent, and validate purpose.
async fn run_onboard(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    use clawz_core::types::ParseOutcome;
    use clawz_worker::purpose::GoalParser;

    let goal_text = body["goal"]
        .as_str()
        .or_else(|| body["description"].as_str())
        .unwrap_or("assist users with their tasks");

    let parser = GoalParser::new();
    let parsed = parser.parse(goal_text);

    let (name, steps) = match parsed {
        ParseOutcome::Parsed(goal) => (
            goal.description.chars().take(48).collect::<String>(),
            vec!["purpose.validate", "agent.create", "identity.init"],
        ),
        ParseOutcome::NeedsClarification(questions) => {
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "status": "needs_clarification",
                    "questions": questions,
                })),
            ));
        }
        ParseOutcome::Failed(reason) => {
            return Err(GatewayError::Unprocessable(reason));
        }
    };

    let model = body["model"]
        .as_str()
        .unwrap_or("claude-sonnet-4-5")
        .to_string();
    let now = Utc::now();
    let record = AgentRecord {
        id: Uuid::new_v4().to_string(),
        name: body["name"].as_str().unwrap_or(&name).to_string(),
        model,
        description: Some(goal_text.to_string()),
        system_prompt: body["system_prompt"].as_str().map(String::from),
        status: AgentStatus::Idle,
        created_at: now,
        updated_at: now,
    };

    state
        .append_audit("system", "onboard", "agent", &record.id, None)
        .await;

    state.agents.write().await.push(record.clone());
    state.persist_agent_record(&record).await;

    if let Some(ref store) = state.identity_store {
        if let Ok(identity) = store.load(&record.id).await {
            let _ = store.save(&identity).await;
        }
    }

    state.publish_event("agent.onboard", json!({ "agent_id": record.id }));

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "onboarded",
            "agent": record,
            "steps_completed": steps,
        })),
    ))
}

/// `POST /agents/orchestrate` — delegate a task across a team.
async fn orchestrate(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let leader = body["leader_agent_id"]
        .as_str()
        .or_else(|| body["agent_id"].as_str())
        .unwrap_or("default");
    let task = body["task"]
        .as_str()
        .unwrap_or("coordinate subtasks")
        .to_string();
    let members: Vec<String> = body["member_agent_ids"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let resp = platform
        .execution
        .orchestrate(OrchestrateRequest {
            leader_agent_id: leader.to_string(),
            task,
            member_agent_ids: members,
            ..Default::default()
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    Ok(Json(json!(resp)))
}

/// `POST /agents/batch` — run the same message against multiple agents.
async fn batch(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let message = body["message"].as_str().unwrap_or("Hello").to_string();
    let agent_ids: Vec<String> = body["agent_ids"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut results = Vec::new();
    for agent_id in &agent_ids {
        let turn = platform
            .execution
            .run_turn(
                agent_id,
                RunTurnRequest {
                    message: message.clone(),
                    model: None,
                    system_prompt: None,
                    conversation_id: None,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        results.push(json!({
            "agent_id": agent_id,
            "content": turn.content,
        }));
    }

    Ok(Json(json!({
        "batch_id": Uuid::new_v4().to_string(),
        "count": results.len(),
        "results": results,
    })))
}

/// `POST /agents/fanout` — parallel multi-model execution.
async fn fanout(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let prompt = body["prompt"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("")
        .to_string();
    let models: Vec<String> = body["models"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let resp = platform
        .execution
        .fan_out(FanOutRequest {
            prompt,
            models,
            strategy: body["strategy"].as_str().map(String::from),
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    Ok(Json(json!(resp)))
}

/// `POST /agents/a2a/discover` — list all registered agent IDs for A2A discovery.
async fn a2a_discover(State(state): State<AppState>) -> Json<Value> {
    let agents = state.agents.read().await;
    let ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    Json(json!({ "agents": ids }))
}

/// `POST /agents/a2a/invoke` — agent-to-agent message via worker runtime.
async fn a2a_invoke(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let from = body["from_agent_id"]
        .as_str()
        .or_else(|| body["from"].as_str())
        .unwrap_or("unknown");
    let to = body["to_agent_id"]
        .as_str()
        .or_else(|| body["to"].as_str())
        .or_else(|| body["agent_id"].as_str())
        .ok_or_else(|| GatewayError::Unprocessable("to_agent_id required".into()))?;
    let message = body["message"].as_str().unwrap_or("").to_string();

    let resp = platform
        .execution
        .a2a_invoke(A2aInvokeRequest {
            from_agent_id: from.to_string(),
            to_agent_id: to.to_string(),
            message,
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    Ok(Json(json!(resp)))
}

/// Bind a phone number provider to an agent (creates a channel + webhook URLs).
#[derive(Debug, Deserialize)]
pub struct BindAgentPhoneBody {
    /// `twilio` or `google_voice`
    pub provider: Option<String>,
    /// E.164 phone number for this agent line
    pub phone_number: Option<String>,
    /// Twilio Account SID
    pub account_sid: Option<String>,
    /// Twilio Auth Token
    pub auth_token: Option<String>,
    /// Google Voice bridge HMAC secret
    pub bridge_secret: Option<String>,
    /// Optional default SMS recipient for tests
    pub default_to: Option<String>,
}

/// `POST /agents/{id}/phone` — register Twilio or Google Voice for direct agent SMS/voice.
async fn bind_agent_phone(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<BindAgentPhoneBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let agents = state.agents.read().await;
    if !agents.iter().any(|a| a.id == agent_id) {
        return Err(GatewayError::not_found("Agent", &agent_id));
    }
    drop(agents);

    let provider = body
        .provider
        .ok_or_else(|| GatewayError::Unprocessable("provider is required".into()))?
        .to_lowercase();
    let phone_number = body
        .phone_number
        .ok_or_else(|| GatewayError::Unprocessable("phone_number is required".into()))?;

    let channel_type = match provider.as_str() {
        "twilio" => "twilio",
        "google_voice" | "google-voice" | "googlevoice" => "google_voice",
        _ => {
            return Err(GatewayError::Unprocessable(format!(
                "unsupported provider: {provider}"
            )));
        }
    };

    let mut config = json!({
        "agent_id": agent_id,
        "phone_number": phone_number,
    });
    if let Some(v) = body.account_sid {
        config["account_sid"] = json!(v);
    }
    if let Some(v) = body.auth_token {
        config["auth_token"] = json!(v);
    }
    if let Some(v) = body.bridge_secret {
        config["bridge_secret"] = json!(v);
    }
    if let Some(v) = body.default_to {
        config["default_to"] = json!(v);
    }

    if channel_type == "twilio"
        && (config.get("account_sid").is_none() || config.get("auth_token").is_none())
    {
        return Err(GatewayError::Unprocessable(
            "Twilio requires account_sid and auth_token".into(),
        ));
    }
    if channel_type == "google_voice" && config.get("bridge_secret").is_none() {
        return Err(GatewayError::Unprocessable(
            "Google Voice requires bridge_secret for webhook verification".into(),
        ));
    }

    let channel_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let record = ChannelRecord {
        id: channel_id.clone(),
        tenant_id: crate::postgres_store::default_tenant(),
        name: format!("{provider} — {phone_number}"),
        channel_type: channel_type.to_string(),
        config,
        enabled: true,
        created_at: now,
        updated_at: now,
    };

    state.channels.write().await.push(record);

    let base = std::env::var("CLAWZ_PUBLIC_URL").unwrap_or_else(|_| "http://localhost:3000".into());
    let base = base.trim_end_matches('/');

    let webhooks = if channel_type == "twilio" {
        json!({
            "sms": format!("{base}/webhooks/twilio/sms/{channel_id}"),
            "voice": format!("{base}/webhooks/twilio/voice/{channel_id}"),
        })
    } else {
        json!({
            "inbound": format!("{base}/webhooks/google-voice/{channel_id}"),
        })
    };

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "agent_id": agent_id,
            "channel_id": channel_id,
            "provider": channel_type,
            "phone_number": phone_number,
            "webhooks": webhooks,
        })),
    ))
}
