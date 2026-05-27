//! Async agent turn execution for multi-participant rooms.

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use clawz_services::dto::RunTurnRequest;

use crate::{AppState, RoomMessageRecord};

use super::rooms::{append_room_message, build_room_snapshot, room_leader_agent_id};

/// Input for deciding whether to queue an agent turn after a user message.
#[derive(Debug, Clone)]
pub struct QueueRoomTurnRequest {
    pub room_id: String,
    pub snapshot: crate::RoomRecord,
    pub content: String,
    pub sender_user_id: String,
    pub mentions: Vec<String>,
    pub visibility: String,
    pub thread_id: Option<String>,
    pub routing_hint: Option<String>,
}

/// Parameters for a queued room agent turn.
#[derive(Debug, Clone)]
pub struct RoomTurnParams {
    pub room_id: String,
    pub tenant_id: String,
    pub entry_agent_id: String,
    pub content: String,
    pub sender_user_id: String,
    pub mentions: Vec<String>,
    pub visibility: String,
    pub thread_id: Option<String>,
    pub room_snapshot: Value,
    pub routing_hint: Option<String>,
}

/// Reserve the room for an in-flight turn. Returns `false` if a turn is already running.
pub async fn try_acquire_room_turn(state: &AppState, room_id: &str) -> bool {
    let mut inflight = state.room_turn_inflight.write().await;
    if inflight.contains(room_id) {
        return false;
    }
    inflight.insert(room_id.to_string());
    true
}

async fn release_room_turn(state: &AppState, room_id: &str) {
    state.room_turn_inflight.write().await.remove(room_id);
}

/// Spawn a background task that runs the agent turn and appends the result to the room.
pub async fn spawn_room_agent_turn(state: AppState, params: RoomTurnParams) {
    let Some(platform) = state.platform.clone() else {
        release_room_turn(&state, &params.room_id).await;
        return;
    };

    let room_id = params.room_id.clone();
    let tenant_id = params.tenant_id.clone();
    let entry_agent = params.entry_agent_id.clone();
    let content = params.content.clone();
    let sender_user_id = params.sender_user_id.clone();
    let routing_hint = params.routing_hint.clone();
    let visibility = params.visibility.clone();
    let thread_id = params.thread_id.clone();
    let room_snapshot = params.room_snapshot.clone();

    tokio::spawn(async move {
        let orchestration_run_id = state.room_runs.read().await.get(&room_id).cloned();

        let turn = platform
            .execution
            .run_turn(
                &entry_agent,
                RunTurnRequest {
                    message: content,
                    model: None,
                    system_prompt: None,
                    conversation_id: Some(room_id.clone()),
                    room_id: Some(room_id.clone()),
                    sender_user_id: Some(sender_user_id),
                    routing_hint,
                    visibility: Some(visibility),
                    room_snapshot: Some(room_snapshot),
                    orchestration_run_id,
                    room_lock_held: true,
                },
            )
            .await;

        match turn {
            Ok(turn) => {
                let agent_msg = RoomMessageRecord {
                    id: turn
                        .message_id
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                    room_id: room_id.clone(),
                    seq: 0,
                    sender_type: "agent".to_string(),
                    sender_id: turn.agent_id.clone(),
                    client_message_id: None,
                    content: turn.content.clone(),
                    mentions: Vec::new(),
                    visibility: "room".to_string(),
                    thread_id: thread_id.clone(),
                    created_at: Utc::now(),
                };
                if let Err(e) = append_room_message(&state, &tenant_id, &room_id, agent_msg).await {
                    tracing::warn!("failed to append agent message for room {room_id}: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("room agent turn failed for {room_id}: {e}");
                let error_msg = RoomMessageRecord {
                    id: Uuid::new_v4().to_string(),
                    room_id: room_id.clone(),
                    seq: 0,
                    sender_type: "system".to_string(),
                    sender_id: "system".to_string(),
                    client_message_id: None,
                    content: format!("Agent turn failed: {e}"),
                    mentions: Vec::new(),
                    visibility: "room".to_string(),
                    thread_id: None,
                    created_at: Utc::now(),
                };
                if let Err(err) = append_room_message(&state, &tenant_id, &room_id, error_msg).await
                {
                    tracing::warn!(
                        "failed to append system error message for room {room_id}: {err}"
                    );
                }
            }
        }

        release_room_turn(&state, &room_id).await;
    });
}

/// Resolve the leader agent for a room snapshot and queue a turn when eligible.
pub async fn queue_room_agent_turn_if_needed(
    state: &AppState,
    req: QueueRoomTurnRequest,
) -> Result<bool, crate::GatewayError> {
    if !super::rooms::should_invoke_agent_turn(&req.snapshot) {
        return Ok(false);
    }

    let Some(entry_agent) = room_leader_agent_id(&req.snapshot) else {
        return Ok(false);
    };

    if state.platform.is_none() {
        return Ok(false);
    }

    if !try_acquire_room_turn(state, &req.room_id).await {
        return Err(crate::GatewayError::Conflict(
            "room turn already in progress".to_string(),
        ));
    }

    let room_snapshot = build_room_snapshot(&req.snapshot);
    spawn_room_agent_turn(
        state.clone(),
        RoomTurnParams {
            room_id: req.room_id.clone(),
            tenant_id: req.snapshot.tenant_id.clone(),
            entry_agent_id: entry_agent,
            content: req.content,
            sender_user_id: req.sender_user_id,
            mentions: req.mentions,
            visibility: req.visibility,
            thread_id: req.thread_id,
            room_snapshot,
            routing_hint: req.routing_hint,
        },
    )
    .await;

    Ok(true)
}
