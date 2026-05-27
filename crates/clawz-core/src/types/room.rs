//! Multi-participant room types — rooms, participants, message semantics, routing.
//!
//! Rooms generalize 1:1 conversations into shared spaces where multiple users
//! and agents collaborate. Side-threads are child rooms linked via `parent_room_id`.
//!
//! // Dependency: persisted by db::RoomRepo, consumed by gateway::routes::rooms and worker::turn_coordinator

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::tenant::TenantId;

// ── Room classification ───────────────────────────────────────────────────────

/// Kind of multi-participant room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomType {
    /// Legacy 1:1 user ↔ agent chat.
    Direct,
    /// One user interacting with an agent team/swarm (1-many).
    AgentTeam,
    /// Multiple users sharing one agent (many-1), optionally with side-threads.
    SharedAgent,
}

impl RoomType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoomType::Direct => "direct",
            RoomType::AgentTeam => "agent_team",
            RoomType::SharedAgent => "shared_agent",
        }
    }
}

/// How agents in a room are orchestrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationMode {
    Single,
    Team,
    Swarm,
}

impl OrchestrationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrchestrationMode::Single => "single",
            OrchestrationMode::Team => "team",
            OrchestrationMode::Swarm => "swarm",
        }
    }
}

/// Swarm collaboration pattern (maps to worker `SwarmPattern`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwarmPattern {
    Hierarchical,
    Pipeline,
    Competitive,
    PeerToPeer,
    Adaptive,
}

impl SwarmPattern {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwarmPattern::Hierarchical => "hierarchical",
            SwarmPattern::Pipeline => "pipeline",
            SwarmPattern::Competitive => "competitive",
            SwarmPattern::PeerToPeer => "peer_to_peer",
            SwarmPattern::Adaptive => "adaptive",
        }
    }
}

/// Who may discover or join a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomVisibility {
    Public,
    Private,
}

impl RoomVisibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoomVisibility::Public => "public",
            RoomVisibility::Private => "private",
        }
    }
}

// ── Participants ──────────────────────────────────────────────────────────────

/// Whether a participant is a human user or an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantType {
    User,
    Agent,
}

impl ParticipantType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ParticipantType::User => "user",
            ParticipantType::Agent => "agent",
        }
    }
}

/// RBAC role within a room (distinct from tenant `Role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantRole {
    Owner,
    Member,
    Leader,
    Observer,
}

impl ParticipantRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ParticipantRole::Owner => "owner",
            ParticipantRole::Member => "member",
            ParticipantRole::Leader => "leader",
            ParticipantRole::Observer => "observer",
        }
    }
}

/// Fine-grained participant capabilities stored in `permissions` JSONB.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParticipantPermissions {
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub invite: bool,
    #[serde(default)]
    pub approve: bool,
    #[serde(default)]
    pub see_internal: bool,
    #[serde(default)]
    pub spend_budget: bool,
}

// ── Messages ────────────────────────────────────────────────────────────────────

/// Semantic category of a room message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    UserText,
    AgentText,
    System,
    Delegation,
    ApprovalRequest,
}

impl MessageKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageKind::UserText => "user_text",
            MessageKind::AgentText => "agent_text",
            MessageKind::System => "system",
            MessageKind::Delegation => "delegation",
            MessageKind::ApprovalRequest => "approval_request",
        }
    }
}

/// Who can see a message in the room transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageVisibility {
    Room,
    Private,
    Internal,
}

impl MessageVisibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageVisibility::Room => "room",
            MessageVisibility::Private => "private",
            MessageVisibility::Internal => "internal",
        }
    }
}

// ── Routing ───────────────────────────────────────────────────────────────────

/// Hint for how a user message should be routed to agents in a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "strategy")]
pub enum RoutingHint {
    Leader,
    Mention { agent_id: String },
    Pattern { pattern: SwarmPattern },
}

// ── Domain structs ──────────────────────────────────────────────────────────────

/// A multi-participant chat room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub conversation_id: Option<Uuid>,
    pub title: Option<String>,
    pub room_type: RoomType,
    pub orchestration_mode: OrchestrationMode,
    pub swarm_pattern: Option<SwarmPattern>,
    pub parent_room_id: Option<Uuid>,
    pub visibility: RoomVisibility,
    pub metadata: serde_json::Value,
    pub primary_agent_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_message_at: DateTime<Utc>,
}

/// Membership row for a room participant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomParticipant {
    pub id: Uuid,
    pub room_id: Uuid,
    pub participant_type: ParticipantType,
    pub participant_id: String,
    pub role: ParticipantRole,
    pub permissions: ParticipantPermissions,
    pub joined_at: DateTime<Utc>,
    pub left_at: Option<DateTime<Utc>>,
}

/// Status of a worker orchestration run bound to a room turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl OrchestrationRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrchestrationRunStatus::Pending => "pending",
            OrchestrationRunStatus::Running => "running",
            OrchestrationRunStatus::Completed => "completed",
            OrchestrationRunStatus::Failed => "failed",
            OrchestrationRunStatus::Cancelled => "cancelled",
        }
    }
}

/// Links a chat turn to worker team/swarm orchestration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationRun {
    pub id: Uuid,
    pub room_id: Uuid,
    pub trigger_message_id: Option<Uuid>,
    pub pattern: Option<SwarmPattern>,
    pub leader_agent_id: Option<Uuid>,
    pub status: OrchestrationRunStatus,
    pub graph_snapshot: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_type_serde_roundtrip() {
        let rt = RoomType::AgentTeam;
        let json = serde_json::to_string(&rt).unwrap();
        assert_eq!(json, "\"agent_team\"");
        let back: RoomType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rt);
    }

    #[test]
    fn routing_hint_mention() {
        let hint = RoutingHint::Mention {
            agent_id: "agent-1".into(),
        };
        let json = serde_json::to_value(&hint).unwrap();
        assert_eq!(json["strategy"], "mention");
        assert_eq!(json["agent_id"], "agent-1");
    }
}
