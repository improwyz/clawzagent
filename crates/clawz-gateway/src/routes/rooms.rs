//! Multi-participant agent room REST API.
//!
//! Rooms support multiple users and agents with sequenced messages, optional
//! private side-threads (Hybrid C), and orchestration binding.

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

use clawz_services::dto::{EvaluateGovernanceRequest, OrchestrateRequest};

use crate::auth::AuthContext;
use crate::routes::room_turn::{self, QueueRoomTurnRequest};
use crate::ws::WsEvent;
use crate::{
    AppState, GatewayError, RoomMessageRecord, RoomParticipantRecord, RoomRecord,
    RoomSideThreadRecord,
};

/// Assemble the rooms sub-router (`/api/v1/rooms`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_rooms).post(create_room))
        .route("/{id}", get(get_room))
        .route("/{id}/participants", post(invite_participant))
        .route("/{id}/messages", get(list_messages).post(send_message))
        .route("/{id}/messages/{message_id}/promote", post(promote_message))
        .route("/{id}/side-threads", post(create_side_thread))
        .route("/{id}/orchestrate", post(orchestrate_room))
}

fn caller_id(auth: Option<Extension<AuthContext>>) -> Result<String, GatewayError> {
    auth.map(|Extension(ctx)| ctx.user_id.clone())
        .ok_or_else(|| GatewayError::Unauthorized("authentication required".to_string()))
}

fn caller_tenant(auth: Option<Extension<AuthContext>>) -> Result<String, GatewayError> {
    auth.map(|Extension(ctx)| ctx.tenant_id.clone())
        .ok_or_else(|| GatewayError::Unauthorized("authentication required".to_string()))
}

fn require_tenant_room(room: &RoomRecord, tenant_id: &str) -> Result<(), GatewayError> {
    if room.tenant_id == tenant_id {
        Ok(())
    } else {
        Err(GatewayError::not_found("Room", &room.id))
    }
}

fn is_member(room: &RoomRecord, user_id: &str) -> bool {
    room.participants
        .iter()
        .any(|p| p.participant_id == user_id)
}

fn require_member(room: &RoomRecord, user_id: &str) -> Result<(), GatewayError> {
    if is_member(room, user_id) {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized(
            "not a member of this room".to_string(),
        ))
    }
}

/// The caller's role in the room, if they are a participant.
fn member_role<'a>(room: &'a RoomRecord, user_id: &str) -> Option<&'a str> {
    room.participants
        .iter()
        .find(|p| p.participant_id == user_id)
        .map(|p| p.role.as_str())
}

/// Authorize granting `role` to a participant.
///
/// Only existing room owners may grant the privileged "owner" role; this closes
/// a privilege-escalation path where any member could invite a participant as
/// "owner".
fn authorize_role_grant(
    room: &RoomRecord,
    caller_id: &str,
    role: &str,
) -> Result<(), GatewayError> {
    if role == "owner" && member_role(room, caller_id) != Some("owner") {
        return Err(GatewayError::Unauthorized(
            "only a room owner can grant the owner role".to_string(),
        ));
    }
    Ok(())
}

fn next_seq(room: &RoomRecord) -> u64 {
    room.messages
        .iter()
        .map(|m| m.seq)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

const VALID_ROOM_TYPES: &[&str] = &["multi_agent", "agent_team", "shared_agent", "direct"];

fn resolve_room_type(explicit: Option<String>, participants: &[RoomParticipantRecord]) -> String {
    if let Some(ref rt) = explicit {
        if VALID_ROOM_TYPES.contains(&rt.as_str()) {
            return rt.clone();
        }
    }
    let agent_count = participants
        .iter()
        .filter(|p| p.participant_type == "agent")
        .count();
    match agent_count {
        0 => explicit.unwrap_or_else(|| "multi_agent".to_string()),
        1 => "shared_agent".to_string(),
        _ => "agent_team".to_string(),
    }
}

pub(crate) fn room_leader_agent_id(room: &RoomRecord) -> Option<String> {
    room.participants
        .iter()
        .find(|p| p.participant_type == "agent" && p.role == "leader")
        .or_else(|| {
            room.participants
                .iter()
                .find(|p| p.participant_type == "agent")
        })
        .map(|p| p.participant_id.clone())
}

pub(crate) fn build_room_snapshot(room: &RoomRecord) -> Value {
    let leader_id = room_leader_agent_id(room).unwrap_or_default();
    let mut metadata = room
        .orchestration_config
        .clone()
        .unwrap_or_else(|| json!({}));
    if !metadata.is_object() {
        metadata = json!({});
    }
    let participants: Vec<Value> = room
        .participants
        .iter()
        .map(|p| {
            let mut entry = json!({
                "participant_id": p.participant_id,
                "participant_type": p.participant_type,
                "role": p.role,
            });
            if p.participant_type == "agent" {
                entry["agent_id"] = json!(p.participant_id);
            }
            entry
        })
        .collect();
    json!({
        "leader_id": leader_id,
        "room_type": room.room_type,
        "metadata": metadata,
        "participants": participants,
    })
}

fn routing_hint_from_mentions(
    room: &RoomRecord,
    mentions: &[String],
) -> Result<Option<String>, GatewayError> {
    for mention in mentions {
        if !is_member(room, mention) {
            return Err(GatewayError::Unprocessable(format!(
                "mention target '{mention}' is not a room participant"
            )));
        }
    }
    Ok(mentions.first().map(|m| format!("mention:{m}")))
}

fn ensure_agent_leader(participants: &mut [RoomParticipantRecord]) {
    let has_leader = participants
        .iter()
        .any(|p| p.participant_type == "agent" && p.role == "leader");
    if has_leader {
        return;
    }
    if let Some(agent) = participants
        .iter_mut()
        .find(|p| p.participant_type == "agent")
    {
        agent.role = "leader".to_string();
    }
}

pub(crate) fn should_invoke_agent_turn(room: &RoomRecord) -> bool {
    matches!(
        room.room_type.as_str(),
        "direct" | "one_many" | "many_one" | "agent_team" | "shared_agent" | "multi_agent"
    ) && room
        .participants
        .iter()
        .any(|p| p.participant_type == "agent")
}

async fn publish_message_append(state: &AppState, room_id: &str, msg: &RoomMessageRecord) {
    state.publish_event(
        "room.message",
        json!({ "room_id": room_id, "message": msg }),
    );
    let ws_event = WsEvent::MessageAppend {
        room_id: room_id.to_string(),
        seq: msg.seq,
        message: serde_json::to_value(msg).unwrap_or(json!({})),
    };
    if let Ok(json) = serde_json::to_value(ws_event) {
        state.publish_room_event(room_id, json).await;
    }
}

fn agent_is_room_participant(room: &RoomRecord, agent_id: &str) -> bool {
    room.participants
        .iter()
        .any(|p| p.participant_type == "agent" && p.participant_id == agent_id)
}

pub(crate) async fn append_room_message(
    state: &AppState,
    tenant_id: &str,
    room_id: &str,
    mut msg: RoomMessageRecord,
) -> Result<Option<RoomMessageRecord>, GatewayError> {
    {
        let rooms = state.rooms.read().await;
        let room = rooms
            .iter()
            .find(|r| r.id == room_id)
            .ok_or_else(|| GatewayError::not_found("Room", room_id))?;
        require_tenant_room(room, tenant_id)?;

        if msg.sender_type == "agent" && !agent_is_room_participant(room, &msg.sender_id) {
            tracing::debug!(
                room_id = room_id,
                agent_id = msg.sender_id,
                "skipping agent message from non-participant subagent"
            );
            return Ok(None);
        }
    }

    if let Some(ref pool) = state.db {
        msg = crate::postgres_store::append_room_message_db(pool, &msg)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    let snapshot = {
        let mut rooms = state.rooms.write().await;
        let room = rooms
            .iter_mut()
            .find(|r| r.id == room_id)
            .ok_or_else(|| GatewayError::not_found("Room", room_id))?;
        require_tenant_room(room, tenant_id)?;

        if msg.seq == 0 {
            msg.seq = next_seq(room);
        }
        room.messages.push(msg.clone());
        room.updated_at = msg.created_at;
        room.clone()
    };

    if let Some(ref pool) = state.db {
        crate::postgres_store::persist_room(pool, &snapshot)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    publish_message_append(state, room_id, &msg).await;

    if snapshot.room_type == "direct" {
        super::conversation_room::mirror_room_message_to_conversation(state, room_id, &msg).await;
    }

    Ok(Some(msg))
}

fn message_visible_to(msg: &RoomMessageRecord, user_id: &str, room: &RoomRecord) -> bool {
    match msg.visibility.as_str() {
        "room" => true,
        "private" => msg.sender_id == user_id || msg.mentions.iter().any(|m| m == user_id),
        "side_thread" => {
            let Some(thread_id) = msg.thread_id.as_ref() else {
                return false;
            };
            room.side_threads
                .iter()
                .find(|t| &t.id == thread_id)
                .map(|t| {
                    t.participant_ids.contains(&user_id.to_string()) || t.created_by == user_id
                })
                .unwrap_or(false)
        }
        _ => true,
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateRoomParticipantInput {
    pub participant_id: Option<String>,
    pub user_id: Option<String>,
    pub agent_id: Option<String>,
    pub participant_type: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRoomBody {
    pub participants: Option<Vec<CreateRoomParticipantInput>>,
    pub room_type: Option<String>,
    pub orchestration_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InviteParticipantBody {
    pub participant_id: Option<String>,
    pub user_id: Option<String>,
    pub agent_id: Option<String>,
    pub participant_type: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListRoomMessagesQuery {
    pub after_seq: Option<u64>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SendRoomMessageBody {
    pub content: Option<String>,
    pub client_message_id: Option<String>,
    pub mentions: Option<Vec<String>>,
    pub visibility: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSideThreadBody {
    pub title: Option<String>,
    pub participant_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct OrchestrateRoomBody {
    pub orchestration_mode: Option<String>,
    pub config: Option<Value>,
    pub agent_ids: Option<Vec<String>>,
}

fn resolve_participant(
    input: &CreateRoomParticipantInput,
) -> Result<(String, String, String), GatewayError> {
    let participant_id = input
        .participant_id
        .clone()
        .or_else(|| input.user_id.clone())
        .or_else(|| input.agent_id.clone())
        .ok_or_else(|| {
            GatewayError::Unprocessable(
                "participant requires participant_id, user_id, or agent_id".to_string(),
            )
        })?;
    let participant_type = if input.agent_id.is_some() {
        "agent".to_string()
    } else {
        input
            .participant_type
            .clone()
            .unwrap_or_else(|| "user".to_string())
    };
    let role = input.role.clone().unwrap_or_else(|| "member".to_string());
    Ok((participant_id, participant_type, role))
}

fn room_json(room: &RoomRecord) -> Value {
    json!({
        "id": room.id,
        "tenant_id": room.tenant_id,
        "room_type": room.room_type,
        "orchestration_mode": room.orchestration_mode,
        "orchestration_config": room.orchestration_config,
        "participants": room.participants,
        "side_threads": room.side_threads,
        "message_count": room.messages.len(),
        "created_by": room.created_by,
        "created_at": room.created_at,
        "updated_at": room.updated_at,
    })
}

/// `POST /rooms` — create a room and seed participants (caller becomes owner).
async fn create_room(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<CreateRoomBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;
    let now = Utc::now();

    let mut participants = vec![RoomParticipantRecord {
        participant_id: user_id.clone(),
        participant_type: "user".to_string(),
        role: "owner".to_string(),
        joined_at: now,
    }];

    if let Some(inputs) = body.participants {
        for input in inputs {
            let (pid, ptype, role) = resolve_participant(&input)?;
            if participants.iter().any(|p| p.participant_id == pid) {
                continue;
            }
            participants.push(RoomParticipantRecord {
                participant_id: pid,
                participant_type: ptype,
                role,
                joined_at: now,
            });
        }
    }

    ensure_agent_leader(&mut participants);

    let room_type = resolve_room_type(body.room_type, &participants);

    let record = RoomRecord {
        id: Uuid::new_v4().to_string(),
        tenant_id,
        room_type,
        orchestration_mode: body.orchestration_mode,
        orchestration_config: None,
        participants,
        messages: Vec::new(),
        side_threads: Vec::new(),
        created_by: user_id,
        created_at: now,
        updated_at: now,
    };

    let mut rooms = state.rooms.write().await;
    rooms.push(record.clone());
    if let Some(ref pool) = state.db {
        crate::postgres_store::persist_room(pool, &record)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    Ok((StatusCode::CREATED, Json(room_json(&record))))
}

/// `GET /rooms` — list rooms where the caller is a participant.
async fn list_rooms(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<Value>, GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;
    let rooms = state.rooms.read().await;
    let visible: Vec<Value> = rooms
        .iter()
        .filter(|room| room.tenant_id == tenant_id && is_member(room, &user_id))
        .map(room_json)
        .collect();
    Ok(Json(json!({ "rooms": visible })))
}

/// `GET /rooms/{id}` — room metadata and participants.
async fn get_room(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;
    let rooms = state.rooms.read().await;
    let room = rooms
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| GatewayError::not_found("Room", &id))?;
    require_tenant_room(room, &tenant_id)?;
    require_member(room, &user_id)?;
    Ok(Json(room_json(room)))
}

/// `POST /rooms/{id}/participants` — invite a user or agent.
async fn invite_participant(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<InviteParticipantBody>,
) -> Result<Json<Value>, GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;
    let input = CreateRoomParticipantInput {
        participant_id: body.participant_id,
        user_id: body.user_id,
        agent_id: body.agent_id,
        participant_type: body.participant_type,
        role: body.role,
    };
    let (pid, ptype, role) = resolve_participant(&input)?;

    let mut rooms = state.rooms.write().await;
    let room = rooms
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| GatewayError::not_found("Room", &id))?;
    require_tenant_room(room, &tenant_id)?;
    require_member(room, &user_id)?;
    authorize_role_grant(room, &user_id, &role)?;

    if room.participants.iter().any(|p| p.participant_id == pid) {
        return Err(GatewayError::Unprocessable(
            "participant already in room".to_string(),
        ));
    }

    let participant = RoomParticipantRecord {
        participant_id: pid.clone(),
        participant_type: ptype,
        role,
        joined_at: Utc::now(),
    };
    room.participants.push(participant.clone());
    room.updated_at = Utc::now();
    let snapshot = room.clone();
    drop(rooms);

    if let Some(ref pool) = state.db {
        crate::postgres_store::persist_room(pool, &snapshot)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    // Audit the privilege-sensitive grant (see authorize_role_grant / IDOR fix).
    state
        .append_audit(
            user_id.clone(),
            "room.participant.invite",
            "room",
            id.clone(),
            Some(format!(
                "participant_id={} role={}",
                participant.participant_id, participant.role
            )),
        )
        .await;

    Ok(Json(json!({ "participant": participant })))
}

/// `GET /rooms/{id}/messages` — paginated history after a sequence cursor.
async fn list_messages(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Query(q): Query<ListRoomMessagesQuery>,
) -> Result<Json<Value>, GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;
    let after_seq = q.after_seq.unwrap_or(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let rooms = state.rooms.read().await;
    let room = rooms
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| GatewayError::not_found("Room", &id))?;
    require_tenant_room(room, &tenant_id)?;
    require_member(room, &user_id)?;

    let visible: Vec<&RoomMessageRecord> = room
        .messages
        .iter()
        .filter(|m| m.seq > after_seq && message_visible_to(m, &user_id, room))
        .take(limit)
        .collect();

    let next_after = visible.last().map(|m| m.seq).unwrap_or(after_seq);

    Ok(Json(json!({
        "room_id": id,
        "messages": visible,
        "after_seq": next_after,
        "has_more": room.messages.iter().any(|m| m.seq > next_after && message_visible_to(m, &user_id, room)),
    })))
}

/// `POST /rooms/{id}/messages` — append a sequenced message and queue an agent turn.
async fn send_message(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<SendRoomMessageBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;
    let content = body
        .content
        .ok_or_else(|| GatewayError::Unprocessable("field 'content' is required".to_string()))?;
    let visibility = body.visibility.unwrap_or_else(|| "room".to_string());

    if visibility == "side_thread" && body.thread_id.is_none() {
        return Err(GatewayError::Unprocessable(
            "thread_id is required for side_thread visibility".to_string(),
        ));
    }

    let mentions = body.mentions.clone().unwrap_or_default();
    {
        let rooms = state.rooms.read().await;
        let room = rooms
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| GatewayError::not_found("Room", &id))?;
        require_tenant_room(room, &tenant_id)?;
        require_member(room, &user_id)?;

        if let Some(ref thread_id) = body.thread_id {
            let thread = room
                .side_threads
                .iter()
                .find(|t| &t.id == thread_id)
                .ok_or_else(|| GatewayError::not_found("SideThread", thread_id))?;
            if !thread.participant_ids.contains(&user_id) && thread.created_by != user_id {
                return Err(GatewayError::Unauthorized(
                    "not a member of this side-thread".to_string(),
                ));
            }
        }

        routing_hint_from_mentions(room, &mentions)?;
    }

    let msg = RoomMessageRecord {
        id: Uuid::new_v4().to_string(),
        room_id: id.clone(),
        seq: 0,
        sender_type: "user".to_string(),
        sender_id: user_id.clone(),
        client_message_id: body.client_message_id,
        content: content.clone(),
        mentions: mentions.clone(),
        visibility: visibility.clone(),
        thread_id: body.thread_id.clone(),
        created_at: Utc::now(),
    };

    let msg = append_room_message(&state, &tenant_id, &id, msg)
        .await?
        .ok_or_else(|| GatewayError::Internal("failed to append room message".to_string()))?;

    let snapshot = {
        let rooms = state.rooms.read().await;
        rooms
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| GatewayError::not_found("Room", &id))?
            .clone()
    };

    let routing_hint = routing_hint_from_mentions(&snapshot, &mentions)?;
    let turn_queued = room_turn::queue_room_agent_turn_if_needed(
        &state,
        QueueRoomTurnRequest {
            room_id: id.clone(),
            snapshot,
            content,
            sender_user_id: user_id,
            mentions,
            visibility,
            thread_id: body.thread_id.clone(),
            routing_hint,
        },
    )
    .await?;

    let status = if turn_queued {
        "turn_queued"
    } else {
        "accepted"
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "message": msg,
            "status": status,
        })),
    ))
}

/// `POST /rooms/{id}/side-threads` — create a private side-thread (Hybrid C).
async fn create_side_thread(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<CreateSideThreadBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;

    let mut participant_ids = body.participant_ids.unwrap_or_default();
    if !participant_ids.contains(&user_id) {
        participant_ids.push(user_id.clone());
    }

    let mut rooms = state.rooms.write().await;
    let room = rooms
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| GatewayError::not_found("Room", &id))?;
    require_tenant_room(room, &tenant_id)?;
    require_member(room, &user_id)?;

    for pid in &participant_ids {
        if !is_member(room, pid) {
            return Err(GatewayError::Unprocessable(format!(
                "participant {pid} is not a room member"
            )));
        }
    }

    let thread = RoomSideThreadRecord {
        id: Uuid::new_v4().to_string(),
        room_id: id.clone(),
        title: body.title,
        participant_ids,
        created_by: user_id,
        created_at: Utc::now(),
    };
    room.side_threads.push(thread.clone());
    room.updated_at = thread.created_at;
    let snapshot = room.clone();
    drop(rooms);

    if let Some(ref pool) = state.db {
        crate::postgres_store::persist_room(pool, &snapshot)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    Ok((StatusCode::CREATED, Json(json!({ "side_thread": thread }))))
}

/// `POST /rooms/{id}/orchestrate` — bind orchestration mode/config to the room.
async fn orchestrate_room(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<OrchestrateRoomBody>,
) -> Result<Json<Value>, GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;
    let run_id = Uuid::new_v4().to_string();

    let mut rooms = state.rooms.write().await;
    let room = rooms
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| GatewayError::not_found("Room", &id))?;
    require_tenant_room(room, &tenant_id)?;
    require_member(room, &user_id)?;

    let is_owner = room
        .participants
        .iter()
        .any(|p| p.participant_id == user_id && p.role == "owner");
    if !is_owner {
        return Err(GatewayError::Unauthorized(
            "only room owners can bind orchestration".to_string(),
        ));
    }

    if let Some(mode) = body.orchestration_mode {
        room.orchestration_mode = Some(mode);
    }
    if let Some(config) = &body.config {
        room.orchestration_config = Some(config.clone());
    }
    room.updated_at = Utc::now();
    let snapshot = room.clone();
    drop(rooms);

    state
        .room_runs
        .write()
        .await
        .insert(id.clone(), run_id.clone());

    if let Some(ref pool) = state.db {
        if let Ok(room_uuid) = Uuid::parse_str(&id) {
            let run_uuid = Uuid::parse_str(&run_id).unwrap_or_else(|_| Uuid::new_v4());
            let leader_uuid =
                room_leader_agent_id(&snapshot).and_then(|s| Uuid::parse_str(&s).ok());
            let now = Utc::now();
            let graph_snapshot = body
                .config
                .clone()
                .unwrap_or_else(|| json!({ "agent_ids": body.agent_ids }));
            let run = clawz_core::db::DbOrchestrationRun {
                id: run_uuid,
                room_id: room_uuid,
                trigger_message_id: None,
                pattern: snapshot.orchestration_mode.clone(),
                leader_agent_id: leader_uuid,
                status: "pending".to_string(),
                graph_snapshot,
                created_at: now,
                updated_at: now,
                completed_at: None,
            };
            let _ = crate::postgres_store::persist_orchestration_run(pool, &run).await;
        }
    }

    if let Some(ref platform) = state.platform {
        let leader = room_leader_agent_id(&snapshot).unwrap_or_else(|| "default".to_string());
        let member_ids: Vec<String> = body.agent_ids.clone().unwrap_or_else(|| {
            snapshot
                .participants
                .iter()
                .filter(|p| p.participant_type == "agent" && p.participant_id != leader)
                .map(|p| p.participant_id.clone())
                .collect()
        });
        let task = body
            .config
            .as_ref()
            .and_then(|c| c.get("task"))
            .and_then(|v| v.as_str())
            .unwrap_or("Coordinate room agents")
            .to_string();

        if let Err(e) = platform
            .execution
            .orchestrate(OrchestrateRequest {
                leader_agent_id: leader,
                task,
                member_agent_ids: member_ids,
                room_id: Some(id.clone()),
                trigger_message_id: None,
            })
            .await
        {
            tracing::warn!("room orchestration failed for {id}: {e}");
        }
    }

    if let Some(ref pool) = state.db {
        crate::postgres_store::persist_room(pool, &snapshot)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    state.publish_event(
        "room.orchestrate",
        json!({
            "room_id": id,
            "run_id": run_id,
            "orchestration_mode": snapshot.orchestration_mode,
            "agent_ids": body.agent_ids,
        }),
    );

    let ws_event = WsEvent::OrchestrationUpdate {
        room_id: id.clone(),
        update: json!({
            "run_id": run_id,
            "orchestration_mode": snapshot.orchestration_mode,
            "orchestration_config": snapshot.orchestration_config,
            "agent_ids": body.agent_ids,
        }),
    };
    if let Ok(json) = serde_json::to_value(ws_event) {
        state.publish_room_event(&id, json).await;
    }

    Ok(Json(json!({
        "room_id": id,
        "run_id": run_id,
        "orchestration_mode": snapshot.orchestration_mode,
        "orchestration_config": snapshot.orchestration_config,
    })))
}

/// `POST /rooms/{id}/messages/{message_id}/promote` — share a private/side-thread message with the room.
async fn promote_message(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path((id, message_id)): Path<(String, String)>,
) -> Result<Json<Value>, GatewayError> {
    let user_id = caller_id(auth.clone())?;
    let tenant_id = caller_tenant(auth)?;

    let (source, room_snapshot) = {
        let rooms = state.rooms.read().await;
        let room = rooms
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| GatewayError::not_found("Room", &id))?;
        require_tenant_room(room, &tenant_id)?;
        require_member(room, &user_id)?;
        let source = room
            .messages
            .iter()
            .find(|m| m.id == message_id)
            .ok_or_else(|| GatewayError::not_found("Message", &message_id))?
            .clone();
        (source, build_room_snapshot(room))
    };

    if source.sender_id != user_id {
        return Err(GatewayError::Unauthorized(
            "only the message author can promote to the room".to_string(),
        ));
    }

    if !matches!(source.visibility.as_str(), "private" | "side_thread") {
        return Err(GatewayError::Unprocessable(
            "only private or side_thread messages can be promoted".to_string(),
        ));
    }

    if let Some(platform) = state.platform.as_ref() {
        if let Some(agent_id) = room_snapshot.get("leader_id").and_then(|v| v.as_str()) {
            let gov = platform
                .execution
                .evaluate_governance(EvaluateGovernanceRequest {
                    agent_id: agent_id.to_string(),
                    action: "promote_message".to_string(),
                    context: json!({
                        "room_id": id,
                        "message_id": message_id,
                        "sender_user_id": user_id,
                        "visibility_from": source.visibility,
                    }),
                })
                .await
                .map_err(|e| GatewayError::Internal(e.to_string()))?;
            if !gov.allowed {
                return Err(GatewayError::Unprocessable(format!(
                    "governance denied promotion: {}",
                    gov.violations
                        .first()
                        .and_then(|v| v.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("policy violation")
                )));
            }
        }
    }

    let promoted = RoomMessageRecord {
        id: Uuid::new_v4().to_string(),
        room_id: id.clone(),
        seq: 0,
        sender_type: source.sender_type.clone(),
        sender_id: source.sender_id.clone(),
        client_message_id: None,
        content: source.content.clone(),
        mentions: source.mentions.clone(),
        visibility: "room".to_string(),
        thread_id: None,
        created_at: Utc::now(),
    };

    let promoted = append_room_message(&state, &tenant_id, &id, promoted)
        .await?
        .ok_or_else(|| GatewayError::Internal("failed to promote room message".to_string()))?;

    Ok(Json(json!({
        "promoted": promoted,
        "source_message_id": message_id,
    })))
}
