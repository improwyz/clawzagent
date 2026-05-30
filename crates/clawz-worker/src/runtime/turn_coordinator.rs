//! Turn coordination for multi-participant agent rooms.
//!
//! Routes inbound room messages to the leader or a mentioned agent, serializes
//! concurrent turns per room, and records delegation events in the room thread.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use clawz_core::traits::MemoryBackend;
use clawz_core::types::message::Message;
use clawz_services::dto::{RunTurnRequest, RunTurnResponse};
use regex::Regex;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::runtime::agent::AgentRuntime;
use crate::runtime::team::{Team, TeamRole};

/// Resolves an agent runtime for a room turn (implemented by [`WorkerService`](crate::service::WorkerService)).
#[async_trait]
pub trait RoomRuntimeProvider: Send + Sync {
    async fn runtime_for_turn(
        &self,
        agent_id: &str,
        req: &RunTurnRequest,
    ) -> Result<Arc<AgentRuntime>>;

    /// Governance, budget, and cost attribution setup before executing a room turn.
    async fn prepare_room_turn(
        &self,
        agent_id: &str,
        req: &RunTurnRequest,
        runtime: &Arc<AgentRuntime>,
    ) -> Result<()>;

    /// Cleanup after a room turn (e.g. clear cost attribution).
    async fn finalize_room_turn(
        &self,
        _agent_id: &str,
        _req: &RunTurnRequest,
        runtime: &Arc<AgentRuntime>,
    ) -> Result<()> {
        runtime.cost_tracker().clear_attribution().await;
        Ok(())
    }
}

/// Coordinates turns in multi-agent rooms: mention routing, per-room locking,
/// team reconstruction from gateway snapshots, and delegation audit events.
pub struct TurnCoordinator {
    room_locks: Arc<RwLock<HashMap<String, bool>>>,
}

impl Default for TurnCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnCoordinator {
    pub fn new() -> Self {
        Self {
            room_locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Run a room-scoped turn: route to the target agent, persist side-thread history
    /// under `room_id`, and return delegation metadata.
    pub async fn run_turn(
        &self,
        entry_agent_id: &str,
        req: RunTurnRequest,
        provider: &dyn RoomRuntimeProvider,
    ) -> Result<RunTurnResponse> {
        let room_id = req
            .room_id
            .clone()
            .ok_or_else(|| ClawzError::Validation("room_id required for room turn".into()))?;

        let lock_held = req.room_lock_held;
        if !lock_held {
            self.acquire_room_lock(&room_id).await;
        }

        let result = self.run_turn_unlocked(entry_agent_id, req, provider).await;

        if !lock_held {
            self.release_room_lock(&room_id).await;
        }
        result
    }

    async fn run_turn_unlocked(
        &self,
        entry_agent_id: &str,
        req: RunTurnRequest,
        provider: &dyn RoomRuntimeProvider,
    ) -> Result<RunTurnResponse> {
        let room_id = req
            .room_id
            .as_ref()
            .ok_or_else(|| ClawzError::Validation("room_id required for room turn".into()))?;

        let (target_agent_id, route_reason) = if req.room_snapshot.is_none() {
            (
                entry_agent_id.to_string(),
                "orchestration:direct".to_string(),
            )
        } else {
            let team = team_from_snapshot(room_id, req.room_snapshot.as_ref()).await?;
            let mentions = parse_mentions(&req.message);
            resolve_route(entry_agent_id, &team, &req, &mentions)?
        };

        let mut delegation_events = Vec::new();
        if target_agent_id != entry_agent_id {
            delegation_events.push(delegation_event(
                "delegation",
                entry_agent_id,
                &target_agent_id,
                &route_reason,
                room_id,
            ));
        }
        delegation_events.push(delegation_event(
            "route",
            entry_agent_id,
            &target_agent_id,
            &route_reason,
            room_id,
        ));

        let conversation_id = req
            .conversation_id
            .clone()
            .unwrap_or_else(|| room_id.clone());

        let runtime = provider.runtime_for_turn(&target_agent_id, &req).await?;
        provider
            .prepare_room_turn(&target_agent_id, &req, &runtime)
            .await?;

        let user_message = room_scoped_user_message(&req, room_id);
        self.record_internal_messages(runtime.memory(), &conversation_id, &delegation_events)
            .await;

        let turn_result = runtime
            .run_in_conversation_with_meta(
                user_message,
                &conversation_id,
                room_turn_metadata(&req, room_id),
            )
            .await;

        provider
            .finalize_room_turn(&target_agent_id, &req, &runtime)
            .await?;

        let reply = turn_result?;
        let content = reply.content.as_text().unwrap_or_default().to_string();
        let message_id = reply.id.to_string();

        Ok(RunTurnResponse {
            agent_id: target_agent_id,
            conversation_id,
            content,
            role: "assistant".to_string(),
            room_id: Some(room_id.clone()),
            message_id: Some(message_id),
            sender_id: req.sender_user_id.clone(),
            delegation_events: Some(delegation_events),
            run_id: None,
        })
    }

    /// Reconstruct a [`Team`] from a gateway room snapshot (async member registration).
    pub async fn build_team_from_snapshot(room_id: &str, snapshot: Option<&Value>) -> Result<Team> {
        team_from_snapshot(room_id, snapshot).await
    }

    pub(crate) async fn acquire_room_lock(&self, room_id: &str) {
        loop {
            {
                let mut locks = self.room_locks.write().await;
                if !locks.get(room_id).copied().unwrap_or(false) {
                    locks.insert(room_id.to_string(), true);
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    pub(crate) async fn release_room_lock(&self, room_id: &str) {
        let mut locks = self.room_locks.write().await;
        locks.remove(room_id);
    }

    async fn record_internal_messages(
        &self,
        memory: Arc<dyn MemoryBackend>,
        conversation_id: &str,
        events: &[Value],
    ) {
        for event in events {
            let text = format!("[room-delegation] {event}");
            let msg = Message::system(text);
            if memory.save_message(conversation_id, &msg).await.is_err() {
                tracing::debug!("room delegation message not persisted for {conversation_id}");
            }
        }
    }
}

/// Parse `@mention` tokens from message text (alphanumeric, underscore, hyphen).
pub fn parse_mentions(text: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"@([a-zA-Z0-9][a-zA-Z0-9_-]*)").expect("mention regex"));
    re.captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

async fn team_from_snapshot(room_id: &str, snapshot: Option<&Value>) -> Result<Team> {
    let snapshot = snapshot.ok_or_else(|| {
        ClawzError::Validation("room_snapshot required for multi-participant room".into())
    })?;

    let leader_id = snapshot
        .get("leader_id")
        .or_else(|| snapshot.pointer("/leader/agent_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ClawzError::Validation("room_snapshot.leader_id missing".into()))?;

    let team = Team::new(room_id, leader_id.clone());

    let participants = snapshot
        .get("participants")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for p in participants {
        let agent_id = p
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let Some(agent_id) = agent_id else {
            continue;
        };
        if agent_id == leader_id {
            continue;
        }
        let role = p
            .get("role")
            .and_then(|v| v.as_str())
            .map(parse_team_role)
            .unwrap_or(TeamRole::Worker);
        team.add_member(agent_id, role).await;
    }

    Ok(team)
}

fn parse_team_role(s: &str) -> TeamRole {
    match s.to_lowercase().as_str() {
        "leader" => TeamRole::Leader,
        "reviewer" => TeamRole::Reviewer,
        "tester" => TeamRole::Tester,
        "documenter" => TeamRole::Documenter,
        "auditor" => TeamRole::Auditor,
        "coordinator" => TeamRole::Coordinator,
        _ => TeamRole::Worker,
    }
}

fn resolve_route(
    entry_agent_id: &str,
    team: &Team,
    req: &RunTurnRequest,
    mentions: &[String],
) -> Result<(String, String)> {
    if let Some(hint) = req.routing_hint.as_deref() {
        if hint == "leader" {
            return Ok((team.leader_id().to_string(), "routing_hint:leader".into()));
        }
        if let Some(id) = hint.strip_prefix("mention:") {
            if participant_exists(team, id, req.room_snapshot.as_ref()) {
                return Ok((id.to_string(), format!("routing_hint:mention:{id}")));
            }
        }
        if let Some(pattern) = hint.strip_prefix("pattern:") {
            if let Some(id) = match_participant_pattern(team, pattern, req.room_snapshot.as_ref()) {
                return Ok((id, format!("routing_hint:pattern:{pattern}")));
            }
        }
    }

    if let Some(mention) = mentions.first() {
        if participant_exists(team, mention, req.room_snapshot.as_ref()) {
            return Ok((mention.clone(), format!("mention:@{mention}")));
        }
    }

    let leader = team.leader_id().to_string();
    let _ = entry_agent_id;
    Ok((leader, "hierarchical:leader".into()))
}

fn participant_exists(team: &Team, id_or_name: &str, snapshot: Option<&Value>) -> bool {
    if id_or_name == team.leader_id() {
        return true;
    }
    let Some(snapshot) = snapshot else {
        return false;
    };
    snapshot
        .get("participants")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|p| {
                p.get("agent_id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| id == id_or_name)
                    || p.get("display_name")
                        .and_then(|v| v.as_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case(id_or_name))
                    || p.get("name")
                        .and_then(|v| v.as_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case(id_or_name))
            })
        })
        .unwrap_or(false)
}

fn match_participant_pattern(
    _team: &Team,
    pattern: &str,
    snapshot: Option<&Value>,
) -> Option<String> {
    let snapshot = snapshot?;
    let participants = snapshot.get("participants")?.as_array()?;
    for p in participants {
        let name = p
            .get("display_name")
            .or_else(|| p.get("name"))
            .and_then(|v| v.as_str());
        let id = p.get("agent_id").and_then(|v| v.as_str());
        if name.is_some_and(|n| n.contains(pattern)) || id.is_some_and(|i| i.contains(pattern)) {
            return id.map(str::to_string);
        }
    }
    None
}

fn room_turn_metadata(req: &RunTurnRequest, room_id: &str) -> Value {
    json!({
        "room_id": room_id,
        "sender_user_id": req.sender_user_id,
        "orchestration_run_id": req.orchestration_run_id,
        "visibility": req.visibility,
    })
}

/// Read optional per-room spend cap from gateway snapshot (`metadata.budget_cap`).
pub fn room_budget_cap(snapshot: Option<&Value>) -> Option<f64> {
    let snapshot = snapshot?;
    snapshot
        .get("metadata")
        .and_then(|m| m.get("budget_cap"))
        .or_else(|| snapshot.get("budget_cap"))
        .and_then(|v| v.as_f64())
        .filter(|cap| *cap > 0.0)
}

fn room_scoped_user_message(req: &RunTurnRequest, room_id: &str) -> Message {
    let visibility = req.visibility.as_deref().unwrap_or("room");
    let sender = req
        .sender_user_id
        .as_deref()
        .map(|s| format!(" sender={s}"))
        .unwrap_or_default();
    let prefixed = format!(
        "[room:{room_id} visibility={visibility}{sender}]\n{}",
        req.message
    );
    Message::user(prefixed)
}

fn delegation_event(event_type: &str, from: &str, to: &str, reason: &str, room_id: &str) -> Value {
    json!({
        "type": event_type,
        "from_agent_id": from,
        "to_agent_id": to,
        "reason": reason,
        "room_id": room_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mentions_extracts_handles() {
        let m = parse_mentions("Hey @alice and @bob-1 can you help?");
        assert_eq!(m, vec!["alice", "bob-1"]);
    }

    #[test]
    fn room_budget_cap_reads_metadata() {
        let snapshot = json!({ "metadata": { "budget_cap": 5.0 } });
        assert_eq!(room_budget_cap(Some(&snapshot)), Some(5.0));
    }

    #[tokio::test]
    async fn team_from_snapshot_builds_leader_and_workers() {
        let snapshot = json!({
            "leader_id": "leader-1",
            "participants": [
                { "agent_id": "leader-1", "role": "leader" },
                { "agent_id": "worker-1", "role": "worker" }
            ]
        });
        let team = team_from_snapshot("room-1", Some(&snapshot)).await.unwrap();
        assert_eq!(team.leader_id(), "leader-1");
        assert_eq!(team.members().await.len(), 2);
    }
}
