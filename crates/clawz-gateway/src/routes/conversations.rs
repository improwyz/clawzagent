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
//! - `send_message` runs a worker turn when the role is `"user"` and persists
//!   messages to Postgres when `DATABASE_URL` is configured.

use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

// Dependency: AppState, ConversationRecord, GatewayError, MessageRecord defined in crate root.
use crate::auth::AuthContext;
use crate::routes::conversation_room;
use crate::{AppState, ConversationRecord, GatewayError};

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
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    let conversations = state.conversations.read().await;
    let rooms = state.rooms.read().await;
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
            let message_count = rooms
                .iter()
                .find(|r| r.id == c.id)
                .filter(|r| !r.messages.is_empty())
                .map(|r| r.messages.len())
                .unwrap_or_else(|| c.messages.len());
            json!({
                "id": c.id,
                "agent_id": c.agent_id,
                "room_id": c.id,
                "title": c.title,
                "archived": c.archived,
                "message_count": message_count,
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
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<CreateConversationBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let agent_id = body
        .agent_id
        .ok_or_else(|| GatewayError::Unprocessable("field 'agent_id' is required".to_string()))?;

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
    if let Some(ref pool) = state.db {
        crate::postgres_store::persist_conversation(pool, &record)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }
    drop(conversations);

    let tenant_id = auth
        .as_ref()
        .map(|Extension(ctx)| ctx.tenant_id.clone())
        .unwrap_or_else(crate::postgres_store::default_tenant);
    let created_by = auth
        .as_ref()
        .map(|Extension(ctx)| ctx.user_id.clone())
        .unwrap_or_else(|| "system".to_string());
    conversation_room::ensure_direct_room(&state, &record, &tenant_id, &created_by).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": record.id,
            "agent_id": record.agent_id,
            "room_id": record.id,
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
    let message_count = {
        let rooms = state.rooms.read().await;
        rooms
            .iter()
            .find(|r| r.id == id)
            .filter(|r| !r.messages.is_empty())
            .map(|r| r.messages.len())
            .unwrap_or_else(|| record.messages.len())
    };
    Ok(Json(json!({
        "id": record.id,
        "agent_id": record.agent_id,
        "room_id": record.id,
        "title": record.title,
        "archived": record.archived,
        "message_count": message_count,
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

/// `GET /conversations/{id}/messages` — list messages from the linked room log.
async fn list_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let messages = conversation_room::list_conversation_messages(&state, &id).await?;
    Ok(Json(json!({
        "conversation_id": id,
        "room_id": id,
        "messages": messages,
        "total": messages.len(),
    })))
}

/// `POST /conversations/{id}/messages` — append via the room pipeline (async agent turns).
async fn send_message(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<SendMessageBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let content = body
        .content
        .ok_or_else(|| GatewayError::Unprocessable("field 'content' is required".to_string()))?;
    let role = body.role.unwrap_or_else(|| "user".to_string());

    let tenant_id = auth
        .as_ref()
        .map(|Extension(ctx)| ctx.tenant_id.clone())
        .unwrap_or_else(crate::postgres_store::default_tenant);
    let sender_user_id = auth
        .as_ref()
        .map(|Extension(ctx)| ctx.user_id.clone())
        .unwrap_or_else(|| "dev".to_string());

    let conversation = {
        let conversations = state.conversations.read().await;
        conversations
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| GatewayError::not_found("Conversation", &id))?
    };

    let (room_msg, turn_queued) = conversation_room::append_conversation_message(
        &state,
        &conversation,
        &tenant_id,
        &sender_user_id,
        &role,
        content,
    )
    .await?;

    let legacy = conversation_room::room_message_to_conversation(&room_msg);
    let status = if turn_queued {
        "turn_queued"
    } else {
        "accepted"
    };

    Ok((
        if role == "user" {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        Json(json!({
            "message": legacy,
            "room_message": room_msg,
            "status": status,
        })),
    ))
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
