//! Bridge legacy 1:1 conversations to the shared room primitive.

use chrono::Utc;
use uuid::Uuid;

use crate::{
    AppState, ConversationRecord, GatewayError, MessageRecord, RoomMessageRecord,
    RoomParticipantRecord, RoomRecord, postgres_store,
};

use super::room_turn::{self, QueueRoomTurnRequest};
use super::rooms::append_room_message;

/// Build a direct room that mirrors a 1:1 conversation (shared ID).
pub fn direct_room_from_conversation(
    conversation: &ConversationRecord,
    tenant_id: &str,
    created_by: &str,
) -> RoomRecord {
    let now = Utc::now();
    let mut participants = vec![RoomParticipantRecord {
        participant_id: conversation.agent_id.clone(),
        participant_type: "agent".to_string(),
        role: "leader".to_string(),
        joined_at: now,
    }];
    if created_by != "system" {
        participants.push(RoomParticipantRecord {
            participant_id: created_by.to_string(),
            participant_type: "user".to_string(),
            role: "owner".to_string(),
            joined_at: now,
        });
    }
    RoomRecord {
        id: conversation.id.clone(),
        tenant_id: tenant_id.to_string(),
        room_type: "direct".to_string(),
        orchestration_mode: Some("single".to_string()),
        orchestration_config: None,
        participants,
        messages: Vec::new(),
        side_threads: Vec::new(),
        created_by: created_by.to_string(),
        created_at: conversation.created_at,
        updated_at: conversation.updated_at,
    }
}

/// Map a room message back to the legacy conversation message shape.
pub fn room_message_to_conversation(room_msg: &RoomMessageRecord) -> MessageRecord {
    let role = match room_msg.sender_type.as_str() {
        "agent" => "assistant",
        "system" => "system",
        _ => "user",
    };
    MessageRecord {
        id: room_msg.id.clone(),
        conversation_id: room_msg.room_id.clone(),
        role: role.into(),
        content: room_msg.content.clone(),
        created_at: room_msg.created_at,
    }
}

fn conversation_role_to_sender_type(role: &str) -> &'static str {
    match role {
        "assistant" => "agent",
        "system" => "system",
        _ => "user",
    }
}

/// Ensure an in-memory (and Postgres) direct room exists for a conversation.
pub async fn ensure_direct_room(
    state: &AppState,
    conversation: &ConversationRecord,
    tenant_id: &str,
    created_by: &str,
) -> Result<RoomRecord, GatewayError> {
    let existing = {
        let rooms = state.rooms.read().await;
        rooms.iter().find(|r| r.id == conversation.id).cloned()
    };

    if let Some(room) = existing {
        if room.messages.is_empty() && !conversation.messages.is_empty() {
            backfill_room_from_conversation(state, conversation, tenant_id, created_by).await?;
        }
        return Ok(state
            .rooms
            .read()
            .await
            .iter()
            .find(|r| r.id == conversation.id)
            .cloned()
            .unwrap_or(room));
    }

    let room = direct_room_from_conversation(conversation, tenant_id, created_by);
    state.rooms.write().await.push(room.clone());

    if let Some(ref pool) = state.db {
        postgres_store::persist_room(pool, &room)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    if !conversation.messages.is_empty() {
        backfill_room_from_conversation(state, conversation, tenant_id, created_by).await?;
    }

    Ok(state
        .rooms
        .read()
        .await
        .iter()
        .find(|r| r.id == conversation.id)
        .cloned()
        .unwrap_or(room))
}

async fn backfill_room_from_conversation(
    state: &AppState,
    conversation: &ConversationRecord,
    tenant_id: &str,
    created_by: &str,
) -> Result<(), GatewayError> {
    for legacy in &conversation.messages {
        let sender_type = conversation_role_to_sender_type(&legacy.role);
        let room_msg = RoomMessageRecord {
            id: legacy.id.clone(),
            room_id: conversation.id.clone(),
            seq: 0,
            sender_type: sender_type.to_string(),
            sender_id: if sender_type == "agent" {
                conversation.agent_id.clone()
            } else if sender_type == "user" {
                created_by.to_string()
            } else {
                "system".to_string()
            },
            client_message_id: None,
            content: legacy.content.clone(),
            mentions: Vec::new(),
            visibility: "room".to_string(),
            thread_id: None,
            created_at: legacy.created_at,
        };
        let _ = append_room_message(state, tenant_id, &conversation.id, room_msg).await?;
    }
    Ok(())
}

/// Mirror a room message into the in-memory conversation cache.
pub async fn mirror_room_message_to_conversation(
    state: &AppState,
    conversation_id: &str,
    room_msg: &RoomMessageRecord,
) {
    let legacy = room_message_to_conversation(room_msg);
    let mut conversations = state.conversations.write().await;
    if let Some(conv) = conversations.iter_mut().find(|c| c.id == conversation_id) {
        if conv.messages.iter().any(|m| m.id == legacy.id) {
            return;
        }
        conv.messages.push(legacy);
        conv.updated_at = room_msg.created_at;
    }
}

/// List messages for a conversation, preferring the authoritative room log.
pub async fn list_conversation_messages(
    state: &AppState,
    conversation_id: &str,
) -> Result<Vec<MessageRecord>, GatewayError> {
    let rooms = state.rooms.read().await;
    if let Some(room) = rooms.iter().find(|r| r.id == conversation_id) {
        if !room.messages.is_empty() {
            return Ok(room
                .messages
                .iter()
                .map(room_message_to_conversation)
                .collect());
        }
    }
    drop(rooms);

    let conversations = state.conversations.read().await;
    conversations
        .iter()
        .find(|c| c.id == conversation_id)
        .map(|c| c.messages.clone())
        .ok_or_else(|| GatewayError::not_found("Conversation", conversation_id))
}

/// Append a message through the room pipeline and mirror it on the conversation.
pub async fn append_conversation_message(
    state: &AppState,
    conversation: &ConversationRecord,
    tenant_id: &str,
    sender_user_id: &str,
    role: &str,
    content: String,
) -> Result<(RoomMessageRecord, bool), GatewayError> {
    ensure_direct_room(state, conversation, tenant_id, sender_user_id).await?;

    let sender_type = conversation_role_to_sender_type(role);
    let sender_id = match sender_type {
        "agent" => conversation.agent_id.clone(),
        "user" => sender_user_id.to_string(),
        _ => "system".to_string(),
    };

    let room_msg = RoomMessageRecord {
        id: Uuid::new_v4().to_string(),
        room_id: conversation.id.clone(),
        seq: 0,
        sender_type: sender_type.to_string(),
        sender_id,
        client_message_id: None,
        content,
        mentions: Vec::new(),
        visibility: "room".to_string(),
        thread_id: None,
        created_at: Utc::now(),
    };

    let saved = append_room_message(state, tenant_id, &conversation.id, room_msg)
        .await?
        .ok_or_else(|| GatewayError::Internal("failed to append room message".to_string()))?;

    mirror_room_message_to_conversation(state, &conversation.id, &saved).await;

    let turn_queued = if role == "user" {
        let snapshot = state
            .rooms
            .read()
            .await
            .iter()
            .find(|r| r.id == conversation.id)
            .cloned()
            .ok_or_else(|| GatewayError::not_found("Room", &conversation.id))?;

        room_turn::queue_room_agent_turn_if_needed(
            state,
            QueueRoomTurnRequest {
                room_id: conversation.id.clone(),
                snapshot,
                content: saved.content.clone(),
                sender_user_id: sender_user_id.to_string(),
                mentions: Vec::new(),
                visibility: "room".to_string(),
                thread_id: None,
                routing_hint: None,
            },
        )
        .await?
    } else {
        false
    };

    Ok((saved, turn_queued))
}
