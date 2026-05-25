//! Conversation and message REST API.
//!
//! A conversation is a threaded message history bound to a single agent.
//! Messages are immutable once appended (except for archival of the whole
//! conversation). This module provides paginated listing, CRUD, message send,
//! and archive capabilities.
//!
//! # Cross-module interactions
//! - `create_conversation` validates that the referenced `agent_id` exists in
//!   `AppState.agents` before creating the thread.
//! - `send_message` auto-generates an assistant reply when the role is `"user"`,
//!   simulating the downstream agent loop that will eventually be wired to real
//!   model inference.

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

// Dependency: AppState, ConversationRecord, GatewayError, MessageRecord defined in crate root.
use crate::{AppState, ConversationRecord, GatewayError, MessageRecord};

/// Assemble the conversation sub-router.
///
/// Mounted by the gateway at `/api/v1/conversations`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_conversations).post(create_conversation))
        .route("/{id}", get(get_conversation).delete(delete_conversation))
        .route("/{id}/messages", get(list_messages).post(send_message))
        .route("/{id}/archive", post(archive_conversation))
}

// ─── Query / body types ───────────────────────────────────────────────────────

/// Query parameters for `GET /conversations`.
#[derive(Debug, Deserialize)]
pub struct ListConversationsQuery {
    /// Filter to conversations owned by this agent ID.
    pub agent_id: Option<String>,
    /// 1-based page number (default 1).
    pub page: Option<usize>,
    /// Items per page, clamped to 1..100 (default 20).
    pub limit: Option<usize>,
}

/// Request body for `POST /conversations`.
#[derive(Debug, Deserialize)]
pub struct CreateConversationBody {
    /// ID of the agent that owns this thread (required).
    pub agent_id: Option<String>,
    /// Optional human-readable title; if omitted the UI can generate one.
    pub title: Option<String>,
}

/// Request body for `POST /conversations/{id}/messages`.
#[derive(Debug, Deserialize)]
pub struct SendMessageBody {
    /// Message role: "user", "assistant", "system", etc. Defaults to "user".
    pub role: Option<String>,
    /// Text payload (required).
    pub content: Option<String>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /conversations` — paginated list with optional agent filter.
///
/// Returns a lightweight projection (`id`, `title`, `message_count`, …)
/// so the payload stays small for UI list views.
async fn list_conversations(
    State(state): State<AppState>,
    Query(q): Query<ListConversationsQuery>,
) -> Json<Value> {
    // Clamp pagination to sensible bounds so a mis-configured client cannot
    // request an unbounded slice that locks the state for too long.
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(20).max(1).min(100);
    let offset = (page - 1) * limit;

    let conversations = state.conversations.read().await;
    let filtered: Vec<&ConversationRecord> = conversations
        .iter()
        .filter(|c| {
            if let Some(ref aid) = q.agent_id {
                &c.agent_id == aid
            } else {
                true
            }
        })
        .collect();

    let total = filtered.len();
    let page_data: Vec<Value> = filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|c| {
            json!({
                "id": c.id,
                "agent_id": c.agent_id,
                "title": c.title,
                "archived": c.archived,
                "message_count": c.messages.len(),
                "created_at": c.created_at,
                "updated_at": c.updated_at,
            })
        })
        .collect();

    Json(json!({
        "data": page_data,
        "total": total,
        "page": page,
        "limit": limit,
    }))
}

/// `POST /conversations` — start a new thread.
///
/// Validates the referenced agent before inserting so dangling references
/// cannot be created through the public API.
async fn create_conversation(
    State(state): State<AppState>,
    Json(body): Json<CreateConversationBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let agent_id = body.agent_id.ok_or_else(|| {
        GatewayError::Unprocessable("field 'agent_id' is required".to_string())
    })?;

    // Verify the agent exists before allocating a conversation.
    // Dependency: reads AppState.agents from the agents domain.
    {
        let agents = state.agents.read().await;
        if !agents.iter().any(|a| a.id == agent_id) {
            return Err(GatewayError::not_found("Agent", &agent_id));
        }
    }

    let now = Utc::now();
    let record = ConversationRecord {
        id: Uuid::new_v4().to_string(),
        agent_id,
        title: body.title,
        archived: false,
        messages: Vec::new(),
        created_at: now,
        updated_at: now,
    };

    let mut conversations = state.conversations.write().await;
    conversations.push(record.clone());

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": record.id,
            "agent_id": record.agent_id,
            "title": record.title,
            "archived": record.archived,
            "message_count": 0,
            "created_at": record.created_at,
            "updated_at": record.updated_at,
        })),
    ))
}

/// `GET /conversations/{id}` — fetch a single conversation projection.
async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let conversations = state.conversations.read().await;
    let record = conversations
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Conversation", &id))?;
    Ok(Json(json!({
        "id": record.id,
        "agent_id": record.agent_id,
        "title": record.title,
        "archived": record.archived,
        "message_count": record.messages.len(),
        "created_at": record.created_at,
        "updated_at": record.updated_at,
    })))
}

/// `DELETE /conversations/{id}` — permanently delete a thread and all its messages.
async fn delete_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mut conversations = state.conversations.write().await;
    let pos = conversations
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Conversation", &id))?;
    conversations.remove(pos);
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /conversations/{id}/messages` — list every message in a thread.
async fn list_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let conversations = state.conversations.read().await;
    let record = conversations
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Conversation", &id))?;
    Ok(Json(json!({
        "conversation_id": id,
        "messages": record.messages,
        "total": record.messages.len(),
    })))
}

/// `POST /conversations/{id}/messages` — append a message.
///
/// When `role` is `"user"` the gateway synthesises an assistant response so the
/// UI can render a complete chat turn immediately. In production this will be
/// replaced by an async inference call to the agent's backing model.
async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SendMessageBody>,
) -> Result<Json<Value>, GatewayError> {
    let content = body.content.ok_or_else(|| {
        GatewayError::Unprocessable("field 'content' is required".to_string())
    })?;
    let role = body.role.unwrap_or_else(|| "user".to_string());

    let now = Utc::now();
    let user_msg = MessageRecord {
        id: Uuid::new_v4().to_string(),
        conversation_id: id.clone(),
        role: role.clone(),
        content: content.clone(),
        created_at: now,
    };

    // Generate assistant response when role is "user" so the conversation
    // feels interactive even before the real inference backend is wired in.
    let assistant_msg = if role == "user" {
        Some(MessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: id.clone(),
            role: "assistant".to_string(),
            // Truncate content to 120 chars so the placeholder response stays readable.
            content: format!("Response to: {}", &content[..content.len().min(120)]),
            created_at: now,
        })
    } else {
        None
    };

    let mut conversations = state.conversations.write().await;
    let record = conversations
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Conversation", &id))?;

    record.messages.push(user_msg.clone());
    let response_msg = if let Some(ref am) = assistant_msg {
        record.messages.push(am.clone());
        Some(am.clone())
    } else {
        None
    };
    record.updated_at = now;

    Ok(Json(json!({
        "message": user_msg,
        "response": response_msg,
    })))
}

/// `POST /conversations/{id}/archive` — soft-delete a thread.
///
/// Archived conversations are excluded from the default list view and from
/// `run_agent` conversation lookups, but the data is retained for compliance.
async fn archive_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let mut conversations = state.conversations.write().await;
    let record = conversations
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| GatewayError::not_found("Conversation", &id))?;
    record.archived = true;
    record.updated_at = Utc::now();
    Ok(Json(json!({ "id": id, "archived": true })))
}
