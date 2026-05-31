//! Database models, SQL migrations, and thin repository helpers.
//!
//! All structs are `sqlx::FromRow` compatible so they can be queried
//! directly with `sqlx::query_as`. The repository structs (`AgentRepo`,
//! `ConversationRepo`, …) are stateless — they take `&PgPool` on every
//! call so the same code works across transactions and connection pools.
//!
//! Schema features:
//!   - `pgvector` extension for 1536-dim embeddings
//!   - JSONB columns for flexible config / metadata
//!   - Foreign-key cascades so deleting an agent cleans up its conversations,
//!     messages, tool executions, costs, governance audits, and memory entries.
//!
//! // Dependency: used by worker::memory, worker::governance, gateway::handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::{ClawzError, Result};

// ── DB model structs ──────────────────────────────────────────────────────────

/// Stored agent record.
/// // Dependency: mapped to table `agents` by AgentRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbAgent {
    pub id: Uuid,
    pub name: String,
    /// Full `AgentConfig` serialised as JSONB.
    pub config: serde_json::Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbAgent {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            config: row.try_get("config")?,
            status: row.try_get("status")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// A conversation session between a user and an agent.
/// // Dependency: mapped to table `conversations` by ConversationRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbConversation {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub channel_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub last_message_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbConversation {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            agent_id: row.try_get("agent_id")?,
            channel_id: row.try_get("channel_id")?,
            started_at: row.try_get("started_at")?,
            last_message_at: row.try_get("last_message_at")?,
            metadata: row.try_get("metadata")?,
        })
    }
}

/// Individual message within a conversation / room.
/// // Dependency: mapped to table `messages` by MessageRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbMessage {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub room_id: Option<Uuid>,
    pub seq: i64,
    pub role: String,
    pub content: serde_json::Value,
    pub sender_type: Option<String>,
    pub sender_id: Option<String>,
    pub message_kind: String,
    pub visibility: String,
    pub parent_message_id: Option<Uuid>,
    pub target_agent_ids: serde_json::Value,
    pub client_message_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbMessage {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            conversation_id: row.try_get("conversation_id")?,
            room_id: row.try_get("room_id").ok(),
            seq: row.try_get("seq").unwrap_or(0),
            role: row.try_get("role")?,
            content: row.try_get("content")?,
            sender_type: row.try_get("sender_type").ok(),
            sender_id: row.try_get("sender_id").ok(),
            message_kind: row
                .try_get("message_kind")
                .unwrap_or_else(|_| "user_text".to_string()),
            visibility: row
                .try_get("visibility")
                .unwrap_or_else(|_| "room".to_string()),
            parent_message_id: row.try_get("parent_message_id").ok(),
            target_agent_ids: row
                .try_get("target_agent_ids")
                .unwrap_or_else(|_| serde_json::json!([])),
            client_message_id: row.try_get("client_message_id").ok(),
            created_at: row.try_get("created_at")?,
        })
    }
}

/// Audit record for a tool execution.
/// // Dependency: mapped to table `tool_executions` by ToolExecutionRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbToolExecution {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub duration_ms: i64,
    pub success: bool,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbToolExecution {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            agent_id: row.try_get("agent_id")?,
            tool_name: row.try_get("tool_name")?,
            input: row.try_get("input")?,
            output: row.try_get("output")?,
            duration_ms: row.try_get("duration_ms")?,
            success: row.try_get("success")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

/// Cost tracking record.
/// // Dependency: mapped to table `cost_records` by CostRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbCostRecord {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub room_id: Option<Uuid>,
    pub orchestration_run_id: Option<Uuid>,
    pub triggered_by_user_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbCostRecord {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            agent_id: row.try_get("agent_id")?,
            room_id: row.try_get("room_id").ok(),
            orchestration_run_id: row.try_get("orchestration_run_id").ok(),
            triggered_by_user_id: row.try_get("triggered_by_user_id").ok(),
            provider: row.try_get("provider")?,
            model: row.try_get("model")?,
            input_tokens: row.try_get("input_tokens")?,
            output_tokens: row.try_get("output_tokens")?,
            cost_usd: row.try_get("cost_usd")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

/// Governance audit trail.
/// // Dependency: mapped to table `governance_audit` by GovernanceAuditRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGovernanceAudit {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub room_id: Option<Uuid>,
    pub orchestration_run_id: Option<Uuid>,
    pub triggered_by_user_id: Option<String>,
    pub action: String,
    pub result: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGovernanceAudit {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            agent_id: row.try_get("agent_id")?,
            room_id: row.try_get("room_id").ok(),
            orchestration_run_id: row.try_get("orchestration_run_id").ok(),
            triggered_by_user_id: row.try_get("triggered_by_user_id").ok(),
            action: row.try_get("action")?,
            result: row.try_get("result")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

/// Multi-participant agent room.
/// // Dependency: mapped to table `rooms` by RoomRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbRoom {
    pub id: Uuid,
    pub tenant_id: String,
    pub conversation_id: Option<Uuid>,
    pub title: Option<String>,
    pub room_type: String,
    pub orchestration_mode: String,
    pub swarm_pattern: Option<String>,
    pub parent_room_id: Option<Uuid>,
    pub visibility: String,
    pub metadata: serde_json::Value,
    pub primary_agent_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_message_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbRoom {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            conversation_id: row.try_get("conversation_id").ok(),
            title: row.try_get("title").ok(),
            room_type: row.try_get("room_type")?,
            orchestration_mode: row.try_get("orchestration_mode")?,
            swarm_pattern: row.try_get("swarm_pattern").ok(),
            parent_room_id: row.try_get("parent_room_id").ok(),
            visibility: row.try_get("visibility")?,
            metadata: row.try_get("metadata")?,
            primary_agent_id: row.try_get("primary_agent_id").ok(),
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            last_message_at: row.try_get("last_message_at")?,
        })
    }
}

/// Room membership row.
/// // Dependency: mapped to table `room_participants` by RoomParticipantRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbRoomParticipant {
    pub id: Uuid,
    pub room_id: Uuid,
    pub participant_type: String,
    pub participant_id: String,
    pub role: String,
    pub permissions: serde_json::Value,
    pub joined_at: DateTime<Utc>,
    pub left_at: Option<DateTime<Utc>>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbRoomParticipant {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            room_id: row.try_get("room_id")?,
            participant_type: row.try_get("participant_type")?,
            participant_id: row.try_get("participant_id")?,
            role: row.try_get("role")?,
            permissions: row.try_get("permissions")?,
            joined_at: row.try_get("joined_at")?,
            left_at: row.try_get("left_at").ok(),
        })
    }
}

/// Worker orchestration run bound to a room turn.
/// // Dependency: mapped to table `orchestration_runs` by OrchestrationRunRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbOrchestrationRun {
    pub id: Uuid,
    pub room_id: Uuid,
    pub trigger_message_id: Option<Uuid>,
    pub pattern: Option<String>,
    pub leader_agent_id: Option<Uuid>,
    pub status: String,
    pub graph_snapshot: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbOrchestrationRun {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            room_id: row.try_get("room_id")?,
            trigger_message_id: row.try_get("trigger_message_id").ok(),
            pattern: row.try_get("pattern").ok(),
            leader_agent_id: row.try_get("leader_agent_id").ok(),
            status: row.try_get("status")?,
            graph_snapshot: row.try_get("graph_snapshot")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            completed_at: row.try_get("completed_at").ok(),
        })
    }
}

/// Deployment record.
/// // Dependency: mapped to table `deployments` by DeploymentRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbDeployment {
    pub id: Uuid,
    pub provider: String,
    pub config: serde_json::Value,
    pub status: String,
    pub url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbDeployment {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            provider: row.try_get("provider")?,
            config: row.try_get("config")?,
            status: row.try_get("status")?,
            url: row.try_get("url")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Fleet worker node registered with the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbFleetNode {
    pub id: Uuid,
    pub name: String,
    pub node_type: String,
    pub host: String,
    pub port: i32,
    pub status: String,
    pub agent_ids: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbFleetNode {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            node_type: row.try_get("node_type")?,
            host: row.try_get("host")?,
            port: row.try_get("port")?,
            status: row.try_get("status")?,
            agent_ids: row.try_get("agent_ids")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Governance policy definition persisted by the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGovernancePolicy {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub rules: serde_json::Value,
    pub enforcement: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGovernancePolicy {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            rules: row.try_get("rules")?,
            enforcement: row.try_get("enforcement")?,
            enabled: row.try_get("enabled")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// LLM provider registration stored by the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGatewayProvider {
    pub id: Uuid,
    pub name: String,
    pub provider_type: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGatewayProvider {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            provider_type: row.try_get("provider_type")?,
            api_key: row.try_get("api_key")?,
            base_url: row.try_get("base_url")?,
            enabled: row.try_get("enabled")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Tool registration stored by the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGatewayTool {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub tool_type: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGatewayTool {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            tool_type: row.try_get("tool_type")?,
            config: row.try_get("config")?,
            enabled: row.try_get("enabled")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Vector memory entry (embedding stored as `vector(1536)` via pgvector).
/// // Dependency: mapped to table `memory_entries` by PgMemoryBackend in worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbMemoryEntry {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub key: String,
    pub value: serde_json::Value,
    /// Raw bytes of the float32 vector (stored via pgvector).
    /// We use Vec<u8> here for portability; callers convert as needed.
    pub embedding: Option<Vec<u8>>,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbMemoryEntry {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            agent_id: row.try_get("agent_id")?,
            key: row.try_get("key")?,
            value: row.try_get("value")?,
            // pgvector columns may not be present in all builds, so fall back to None.
            embedding: row.try_get("embedding").ok(),
            created_at: row.try_get("created_at")?,
        })
    }
}

// ── SQL migrations ────────────────────────────────────────────────────────────

/// All DDL statements needed to initialise the ClawZ schema.
/// This is idempotent (uses `IF NOT EXISTS` everywhere).
pub const MIGRATIONS_SQL: &str = r#"
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS agents (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    config      JSONB NOT NULL DEFAULT '{}',
    status      TEXT NOT NULL DEFAULT 'idle',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(status);

CREATE TABLE IF NOT EXISTS conversations (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id         UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    channel_id       UUID,
    started_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_message_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata         JSONB NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_conversations_agent ON conversations(agent_id);
CREATE INDEX IF NOT EXISTS idx_conversations_last_msg ON conversations(last_message_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    content         JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id, created_at);

CREATE TABLE IF NOT EXISTS tool_executions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id    UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    tool_name   TEXT NOT NULL,
    input       JSONB NOT NULL DEFAULT '{}',
    output      JSONB NOT NULL DEFAULT '{}',
    duration_ms BIGINT NOT NULL DEFAULT 0,
    success     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tool_executions_agent ON tool_executions(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tool_executions_name ON tool_executions(tool_name);

CREATE TABLE IF NOT EXISTS cost_records (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id      UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL,
    model         TEXT NOT NULL,
    input_tokens  BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    cost_usd      DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_cost_records_agent ON cost_records(agent_id, created_at DESC);

CREATE TABLE IF NOT EXISTS governance_audit (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id   UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    action     TEXT NOT NULL,
    result     JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_governance_audit_agent ON governance_audit(agent_id, created_at DESC);

CREATE TABLE IF NOT EXISTS deployments (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider   TEXT NOT NULL,
    config     JSONB NOT NULL DEFAULT '{}',
    status     TEXT NOT NULL DEFAULT 'pending',
    url        TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_deployments_status ON deployments(status);

CREATE TABLE IF NOT EXISTS memory_entries (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id   UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value      JSONB NOT NULL DEFAULT '{}',
    embedding  vector(1536),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(agent_id, key)
);

CREATE INDEX IF NOT EXISTS idx_memory_entries_agent ON memory_entries(agent_id);

CREATE TABLE IF NOT EXISTS fleet_nodes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    node_type   TEXT NOT NULL DEFAULT 'worker',
    host        TEXT NOT NULL,
    port        INT NOT NULL DEFAULT 8080,
    status      TEXT NOT NULL DEFAULT 'online',
    agent_ids   JSONB NOT NULL DEFAULT '[]',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_fleet_nodes_status ON fleet_nodes(status);

CREATE TABLE IF NOT EXISTS governance_policies (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    rules       JSONB NOT NULL DEFAULT '[]',
    enforcement TEXT NOT NULL DEFAULT 'audit',
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_governance_policies_enabled ON governance_policies(enabled);

CREATE TABLE IF NOT EXISTS gateway_providers (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name          TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    api_key       TEXT,
    base_url      TEXT,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS gateway_tools (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    tool_type   TEXT NOT NULL DEFAULT 'function',
    config      JSONB NOT NULL DEFAULT '{}',
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_gateway_tools_name ON gateway_tools(name);

CREATE TABLE IF NOT EXISTS gateway_audit (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor         TEXT NOT NULL,
    action        TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id   TEXT NOT NULL,
    details       TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_gateway_audit_created ON gateway_audit(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_gateway_audit_resource ON gateway_audit(resource_type, resource_id);

CREATE TABLE IF NOT EXISTS rooms (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          TEXT NOT NULL DEFAULT 'default',
    conversation_id    UUID UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    title              TEXT,
    room_type          TEXT NOT NULL DEFAULT 'direct',
    orchestration_mode TEXT NOT NULL DEFAULT 'single',
    swarm_pattern      TEXT,
    parent_room_id     UUID REFERENCES rooms(id) ON DELETE SET NULL,
    visibility         TEXT NOT NULL DEFAULT 'public',
    metadata           JSONB NOT NULL DEFAULT '{}',
    primary_agent_id   UUID REFERENCES agents(id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_message_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_rooms_tenant ON rooms(tenant_id);
CREATE INDEX IF NOT EXISTS idx_rooms_parent ON rooms(parent_room_id);
CREATE INDEX IF NOT EXISTS idx_rooms_last_msg ON rooms(last_message_at DESC);
CREATE INDEX IF NOT EXISTS idx_rooms_type ON rooms(room_type);

CREATE TABLE IF NOT EXISTS room_participants (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    room_id          UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    participant_type TEXT NOT NULL,
    participant_id   TEXT NOT NULL,
    role             TEXT NOT NULL DEFAULT 'member',
    permissions      JSONB NOT NULL DEFAULT '{}',
    joined_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    left_at          TIMESTAMPTZ,
    UNIQUE (room_id, participant_type, participant_id)
);

CREATE INDEX IF NOT EXISTS idx_room_participants_room ON room_participants(room_id);
CREATE INDEX IF NOT EXISTS idx_room_participants_lookup
    ON room_participants(participant_type, participant_id);

ALTER TABLE messages ADD COLUMN IF NOT EXISTS room_id UUID REFERENCES rooms(id) ON DELETE CASCADE;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS seq BIGINT NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS sender_type TEXT;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS sender_id TEXT;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS message_kind TEXT NOT NULL DEFAULT 'user_text';
ALTER TABLE messages ADD COLUMN IF NOT EXISTS visibility TEXT NOT NULL DEFAULT 'room';
ALTER TABLE messages ADD COLUMN IF NOT EXISTS parent_message_id UUID REFERENCES messages(id) ON DELETE SET NULL;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS target_agent_ids JSONB NOT NULL DEFAULT '[]';
ALTER TABLE messages ADD COLUMN IF NOT EXISTS client_message_id UUID;

CREATE INDEX IF NOT EXISTS idx_messages_room_seq ON messages(room_id, seq);
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_room_client_id
    ON messages(room_id, client_message_id)
    WHERE client_message_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS orchestration_runs (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    room_id            UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    trigger_message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    pattern            TEXT,
    leader_agent_id    UUID REFERENCES agents(id) ON DELETE SET NULL,
    status             TEXT NOT NULL DEFAULT 'pending',
    graph_snapshot     JSONB NOT NULL DEFAULT '{}',
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at       TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_orchestration_runs_room ON orchestration_runs(room_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_orchestration_runs_status ON orchestration_runs(status);

ALTER TABLE cost_records ADD COLUMN IF NOT EXISTS room_id UUID REFERENCES rooms(id) ON DELETE SET NULL;
ALTER TABLE cost_records ADD COLUMN IF NOT EXISTS orchestration_run_id UUID REFERENCES orchestration_runs(id) ON DELETE SET NULL;
ALTER TABLE cost_records ADD COLUMN IF NOT EXISTS triggered_by_user_id TEXT;

CREATE INDEX IF NOT EXISTS idx_cost_records_room ON cost_records(room_id, created_at DESC);

ALTER TABLE governance_audit ADD COLUMN IF NOT EXISTS room_id UUID REFERENCES rooms(id) ON DELETE SET NULL;
ALTER TABLE governance_audit ADD COLUMN IF NOT EXISTS orchestration_run_id UUID REFERENCES orchestration_runs(id) ON DELETE SET NULL;
ALTER TABLE governance_audit ADD COLUMN IF NOT EXISTS triggered_by_user_id TEXT;

CREATE INDEX IF NOT EXISTS idx_governance_audit_room ON governance_audit(room_id, created_at DESC);

INSERT INTO rooms (
    id,
    tenant_id,
    conversation_id,
    title,
    room_type,
    orchestration_mode,
    visibility,
    metadata,
    primary_agent_id,
    created_at,
    updated_at,
    last_message_at
)
SELECT
    c.id,
    COALESCE(c.metadata->>'tenant_id', 'default'),
    c.id,
    c.metadata->>'title',
    'direct',
    'single',
    'public',
    c.metadata,
    c.agent_id,
    c.started_at,
    c.started_at,
    c.last_message_at
FROM conversations c
WHERE NOT EXISTS (
    SELECT 1 FROM rooms r WHERE r.conversation_id = c.id
);

INSERT INTO room_participants (room_id, participant_type, participant_id, role, permissions)
SELECT
    r.id,
    'agent',
    r.primary_agent_id::text,
    'leader',
    '{}'::jsonb
FROM rooms r
WHERE r.primary_agent_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM room_participants rp
      WHERE rp.room_id = r.id
        AND rp.participant_type = 'agent'
        AND rp.participant_id = r.primary_agent_id::text
  );

UPDATE messages m
SET room_id = m.conversation_id
WHERE m.room_id IS NULL
  AND EXISTS (SELECT 1 FROM rooms r WHERE r.id = m.conversation_id);

CREATE TABLE IF NOT EXISTS gateway_users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL DEFAULT 'default',
    email         TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'user',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_gateway_users_tenant_email
    ON gateway_users(tenant_id, email);

CREATE TABLE IF NOT EXISTS gateway_api_keys (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id  TEXT NOT NULL DEFAULT 'default',
    user_id    UUID NOT NULL REFERENCES gateway_users(id) ON DELETE CASCADE,
    key_hash   TEXT NOT NULL,
    label      TEXT NOT NULL DEFAULT 'default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_gateway_api_keys_user ON gateway_api_keys(user_id);

CREATE TABLE IF NOT EXISTS gateway_channels (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    name         TEXT NOT NULL,
    channel_type TEXT NOT NULL,
    config       JSONB NOT NULL DEFAULT '{}',
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_gateway_channels_tenant ON gateway_channels(tenant_id);

-- Distributed (cross-fleet) rate-limit counters: fixed-window hit counts keyed
-- by "{class}:{actor}". The per-node in-memory bucket is the fast path; this
-- table enforces a shared quota across all gateway nodes (fail-open on error).
CREATE TABLE IF NOT EXISTS rate_limit_counters (
    bucket       TEXT NOT NULL,
    window_start BIGINT NOT NULL,
    hits         BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (bucket, window_start)
);

CREATE INDEX IF NOT EXISTS idx_rate_limit_counters_window ON rate_limit_counters(window_start);

-- Idempotency keys for non-idempotent POSTs: stores the first response so a
-- retried request with the same Idempotency-Key replays it instead of acting
-- twice. Rows are pruned past their TTL.
CREATE TABLE IF NOT EXISTS idempotency_keys (
    id           TEXT PRIMARY KEY,
    method       TEXT NOT NULL,
    path         TEXT NOT NULL,
    status_code  INT NOT NULL,
    response     JSONB NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_idempotency_keys_created ON idempotency_keys(created_at);
"#;

// ── run_migrations ────────────────────────────────────────────────────────────

/// Execute all DDL migrations against the connected pool.
/// This is idempotent (uses `IF NOT EXISTS` everywhere).
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    // Strip `--` line comments before splitting on `;`. A naive split would
    // break a statement whenever a comment contains a semicolon; the controlled
    // DDL below has no string literals containing `--`, so this is safe.
    let cleaned: String = MIGRATIONS_SQL
        .lines()
        .map(|line| match line.find("--") {
            Some(idx) => &line[..idx],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for statement in cleaned.split(';') {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        sqlx::query(trimmed)
            .execute(pool)
            .await
            .map_err(|e| ClawzError::Database(format!("migration failed: {e}\nSQL: {trimmed}")))?;
    }
    Ok(())
}

// ── Repository helpers ────────────────────────────────────────────────────────

/// Thin repository layer for `DbAgent`.
/// // Called by: worker::agent_manager, gateway::admin_handlers
pub struct AgentRepo;

impl AgentRepo {
    pub async fn insert(pool: &PgPool, agent: &DbAgent) -> Result<()> {
        sqlx::query(
            "INSERT INTO agents (id, name, config, status, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(agent.id)
        .bind(&agent.name)
        .bind(&agent.config)
        .bind(&agent.status)
        .bind(agent.created_at)
        .bind(agent.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<DbAgent>> {
        let row = sqlx::query_as::<_, DbAgent>("SELECT * FROM agents WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        Ok(row)
    }

    pub async fn list(pool: &PgPool, limit: i64, offset: i64) -> Result<Vec<DbAgent>> {
        let rows = sqlx::query_as::<_, DbAgent>(
            "SELECT * FROM agents ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn update_status(pool: &PgPool, id: Uuid, status: &str) -> Result<()> {
        sqlx::query("UPDATE agents SET status = $1, updated_at = NOW() WHERE id = $2")
            .bind(status)
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM agents WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// Thin repository layer for `DbConversation`.
/// // Called by: worker::conversation_manager
pub struct ConversationRepo;

impl ConversationRepo {
    pub async fn insert(pool: &PgPool, conv: &DbConversation) -> Result<()> {
        Self::upsert(pool, conv).await
    }

    /// Insert or update conversation metadata (idempotent gateway persistence).
    pub async fn upsert(pool: &PgPool, conv: &DbConversation) -> Result<()> {
        sqlx::query(
            "INSERT INTO conversations \
             (id, agent_id, channel_id, started_at, last_message_at, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (id) DO UPDATE SET \
               last_message_at = EXCLUDED.last_message_at, \
               metadata = EXCLUDED.metadata, \
               agent_id = EXCLUDED.agent_id",
        )
        .bind(conv.id)
        .bind(conv.agent_id)
        .bind(conv.channel_id)
        .bind(conv.started_at)
        .bind(conv.last_message_at)
        .bind(&conv.metadata)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<DbConversation>> {
        let row = sqlx::query_as::<_, DbConversation>("SELECT * FROM conversations WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        Ok(row)
    }

    pub async fn list_for_agent(
        pool: &PgPool,
        agent_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DbConversation>> {
        let rows = sqlx::query_as::<_, DbConversation>(
            "SELECT * FROM conversations \
             WHERE agent_id = $1 \
             ORDER BY last_message_at DESC \
             LIMIT $2",
        )
        .bind(agent_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// Bump `last_message_at` to NOW() for a conversation.
    pub async fn touch(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE conversations SET last_message_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// List recent conversations across all agents (gateway hydration).
    pub async fn list_recent(pool: &PgPool, limit: i64) -> Result<Vec<DbConversation>> {
        let rows = sqlx::query_as::<_, DbConversation>(
            "SELECT * FROM conversations ORDER BY last_message_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

/// Thin repository layer for `DbRoom`.
/// // Called by: gateway::rooms_handlers, worker::turn_coordinator
pub struct RoomRepo;

impl RoomRepo {
    pub async fn insert(pool: &PgPool, room: &DbRoom) -> Result<()> {
        sqlx::query(
            "INSERT INTO rooms \
             (id, tenant_id, conversation_id, title, room_type, orchestration_mode, swarm_pattern, \
              parent_room_id, visibility, metadata, primary_agent_id, created_at, updated_at, last_message_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(room.id)
        .bind(&room.tenant_id)
        .bind(room.conversation_id)
        .bind(&room.title)
        .bind(&room.room_type)
        .bind(&room.orchestration_mode)
        .bind(&room.swarm_pattern)
        .bind(room.parent_room_id)
        .bind(&room.visibility)
        .bind(&room.metadata)
        .bind(room.primary_agent_id)
        .bind(room.created_at)
        .bind(room.updated_at)
        .bind(room.last_message_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<DbRoom>> {
        let row = sqlx::query_as::<_, DbRoom>("SELECT * FROM rooms WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        Ok(row)
    }

    pub async fn list_for_tenant(
        pool: &PgPool,
        tenant_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<DbRoom>> {
        let rows = sqlx::query_as::<_, DbRoom>(
            "SELECT * FROM rooms \
             WHERE tenant_id = $1 \
             ORDER BY last_message_at DESC \
             LIMIT $2 OFFSET $3",
        )
        .bind(tenant_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// List recent rooms across all tenants (gateway startup hydration).
    pub async fn list_recent(pool: &PgPool, limit: i64) -> Result<Vec<DbRoom>> {
        let rows = sqlx::query_as::<_, DbRoom>(
            "SELECT * FROM rooms ORDER BY last_message_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// Insert or update a room header row (gateway best-effort persistence).
    pub async fn upsert(pool: &PgPool, room: &DbRoom) -> Result<()> {
        sqlx::query(
            "INSERT INTO rooms \
             (id, tenant_id, conversation_id, title, room_type, orchestration_mode, swarm_pattern, \
              parent_room_id, visibility, metadata, primary_agent_id, created_at, updated_at, last_message_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
             ON CONFLICT (id) DO UPDATE SET \
               title = EXCLUDED.title, \
               room_type = EXCLUDED.room_type, \
               orchestration_mode = EXCLUDED.orchestration_mode, \
               swarm_pattern = EXCLUDED.swarm_pattern, \
               visibility = EXCLUDED.visibility, \
               metadata = EXCLUDED.metadata, \
               primary_agent_id = EXCLUDED.primary_agent_id, \
               updated_at = EXCLUDED.updated_at, \
               last_message_at = EXCLUDED.last_message_at",
        )
        .bind(room.id)
        .bind(&room.tenant_id)
        .bind(room.conversation_id)
        .bind(&room.title)
        .bind(&room.room_type)
        .bind(&room.orchestration_mode)
        .bind(&room.swarm_pattern)
        .bind(room.parent_room_id)
        .bind(&room.visibility)
        .bind(&room.metadata)
        .bind(room.primary_agent_id)
        .bind(room.created_at)
        .bind(room.updated_at)
        .bind(room.last_message_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Add a participant to a room (idempotent on unique key).
    pub async fn add_participant(pool: &PgPool, participant: &DbRoomParticipant) -> Result<()> {
        RoomParticipantRepo::insert(pool, participant).await
    }

    /// Insert a message with server-assigned `seq` and bump room activity timestamp.
    ///
    /// When `client_message_id` is set and already exists for the room, returns the
    /// existing message sequence without inserting a duplicate.
    pub async fn append_message(pool: &PgPool, msg: &mut DbMessage) -> Result<i64> {
        let room_id = msg
            .room_id
            .ok_or_else(|| ClawzError::Database("append_message requires room_id".to_string()))?;

        if let Some(client_id) = msg.client_message_id {
            if let Some(existing) = sqlx::query_as::<_, DbMessage>(
                "SELECT * FROM messages WHERE room_id = $1 AND client_message_id = $2",
            )
            .bind(room_id)
            .bind(client_id)
            .fetch_optional(pool)
            .await?
            {
                msg.seq = existing.seq;
                msg.id = existing.id;
                return Ok(existing.seq);
            }
        }

        let mut tx = pool.begin().await?;

        let next_seq: (i64,) =
            sqlx::query_as("SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE room_id = $1")
                .bind(room_id)
                .fetch_one(&mut *tx)
                .await?;

        msg.seq = next_seq.0;

        sqlx::query(
            "INSERT INTO messages \
             (id, conversation_id, room_id, seq, role, content, sender_type, sender_id, \
              message_kind, visibility, parent_message_id, target_agent_ids, client_message_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(msg.id)
        .bind(msg.conversation_id)
        .bind(msg.room_id)
        .bind(msg.seq)
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(&msg.sender_type)
        .bind(&msg.sender_id)
        .bind(&msg.message_kind)
        .bind(&msg.visibility)
        .bind(msg.parent_message_id)
        .bind(&msg.target_agent_ids)
        .bind(msg.client_message_id)
        .bind(msg.created_at)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE rooms SET last_message_at = $1, updated_at = $1 WHERE id = $2")
            .bind(msg.created_at)
            .bind(room_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(msg.seq)
    }
}

/// Thin repository layer for `DbRoomParticipant`.
pub struct RoomParticipantRepo;

impl RoomParticipantRepo {
    pub async fn insert(pool: &PgPool, participant: &DbRoomParticipant) -> Result<()> {
        sqlx::query(
            "INSERT INTO room_participants \
             (id, room_id, participant_type, participant_id, role, permissions, joined_at, left_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (room_id, participant_type, participant_id) DO UPDATE SET \
               role = EXCLUDED.role, \
               permissions = EXCLUDED.permissions, \
               left_at = EXCLUDED.left_at",
        )
        .bind(participant.id)
        .bind(participant.room_id)
        .bind(&participant.participant_type)
        .bind(&participant.participant_id)
        .bind(&participant.role)
        .bind(&participant.permissions)
        .bind(participant.joined_at)
        .bind(participant.left_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list_for_room(pool: &PgPool, room_id: Uuid) -> Result<Vec<DbRoomParticipant>> {
        let rows = sqlx::query_as::<_, DbRoomParticipant>(
            "SELECT * FROM room_participants \
             WHERE room_id = $1 AND left_at IS NULL \
             ORDER BY joined_at ASC",
        )
        .bind(room_id)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

/// Thin repository layer for `DbOrchestrationRun`.
pub struct OrchestrationRunRepo;

impl OrchestrationRunRepo {
    pub async fn insert(pool: &PgPool, run: &DbOrchestrationRun) -> Result<()> {
        sqlx::query(
            "INSERT INTO orchestration_runs \
             (id, room_id, trigger_message_id, pattern, leader_agent_id, status, graph_snapshot, \
              created_at, updated_at, completed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(run.id)
        .bind(run.room_id)
        .bind(run.trigger_message_id)
        .bind(&run.pattern)
        .bind(run.leader_agent_id)
        .bind(&run.status)
        .bind(&run.graph_snapshot)
        .bind(run.created_at)
        .bind(run.updated_at)
        .bind(run.completed_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<DbOrchestrationRun>> {
        let row = sqlx::query_as::<_, DbOrchestrationRun>(
            "SELECT * FROM orchestration_runs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn list_for_room(
        pool: &PgPool,
        room_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DbOrchestrationRun>> {
        let rows = sqlx::query_as::<_, DbOrchestrationRun>(
            "SELECT * FROM orchestration_runs \
             WHERE room_id = $1 \
             ORDER BY created_at DESC \
             LIMIT $2",
        )
        .bind(room_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

/// Thin repository layer for `DbMessage`.
/// // Called by: worker::conversation_manager, worker::memory backends
pub struct MessageRepo;

impl MessageRepo {
    pub async fn insert(pool: &PgPool, msg: &DbMessage) -> Result<()> {
        sqlx::query(
            "INSERT INTO messages \
             (id, conversation_id, room_id, seq, role, content, sender_type, sender_id, \
              message_kind, visibility, parent_message_id, target_agent_ids, client_message_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(msg.id)
        .bind(msg.conversation_id)
        .bind(msg.room_id)
        .bind(msg.seq)
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(&msg.sender_type)
        .bind(&msg.sender_id)
        .bind(&msg.message_kind)
        .bind(&msg.visibility)
        .bind(msg.parent_message_id)
        .bind(&msg.target_agent_ids)
        .bind(msg.client_message_id)
        .bind(msg.created_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list_for_room(
        pool: &PgPool,
        room_id: Uuid,
        after_seq: i64,
        limit: i64,
    ) -> Result<Vec<DbMessage>> {
        let rows = sqlx::query_as::<_, DbMessage>(
            "SELECT * FROM messages \
             WHERE room_id = $1 AND seq > $2 \
             ORDER BY seq ASC \
             LIMIT $3",
        )
        .bind(room_id)
        .bind(after_seq)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_for_conversation(
        pool: &PgPool,
        conversation_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DbMessage>> {
        let rows = sqlx::query_as::<_, DbMessage>(
            "SELECT * FROM messages \
             WHERE conversation_id = $1 \
             ORDER BY created_at ASC \
             LIMIT $2",
        )
        .bind(conversation_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn count_for_conversation(pool: &PgPool, conversation_id: Uuid) -> Result<i64> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*)::BIGINT FROM messages WHERE conversation_id = $1")
                .bind(conversation_id)
                .fetch_one(pool)
                .await?;
        Ok(row.0)
    }
}

/// Thin repository layer for `DbToolExecution`.
/// // Called by: worker::tool_orchestrator after each tool run.
pub struct ToolExecutionRepo;

impl ToolExecutionRepo {
    pub async fn insert(pool: &PgPool, record: &DbToolExecution) -> Result<()> {
        sqlx::query(
            "INSERT INTO tool_executions \
             (id, agent_id, tool_name, input, output, duration_ms, success, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(record.id)
        .bind(record.agent_id)
        .bind(&record.tool_name)
        .bind(&record.input)
        .bind(&record.output)
        .bind(record.duration_ms)
        .bind(record.success)
        .bind(record.created_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list_for_agent(
        pool: &PgPool,
        agent_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DbToolExecution>> {
        let rows = sqlx::query_as::<_, DbToolExecution>(
            "SELECT * FROM tool_executions \
             WHERE agent_id = $1 \
             ORDER BY created_at DESC \
             LIMIT $2",
        )
        .bind(agent_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

/// Thin repository layer for `DbCostRecord`.
/// // Called by: worker::provider implementations after each LLM request.
pub struct CostRepo;

impl CostRepo {
    pub async fn insert(pool: &PgPool, record: &DbCostRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO cost_records \
             (id, agent_id, room_id, orchestration_run_id, triggered_by_user_id, \
              provider, model, input_tokens, output_tokens, cost_usd, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(record.id)
        .bind(record.agent_id)
        .bind(record.room_id)
        .bind(record.orchestration_run_id)
        .bind(&record.triggered_by_user_id)
        .bind(&record.provider)
        .bind(&record.model)
        .bind(record.input_tokens)
        .bind(record.output_tokens)
        .bind(record.cost_usd)
        .bind(record.created_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Return the total cost for `agent_id` since `since`.
    pub async fn total_since(pool: &PgPool, agent_id: Uuid, since: DateTime<Utc>) -> Result<f64> {
        let row: (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(cost_usd), 0.0) \
             FROM cost_records \
             WHERE agent_id = $1 AND created_at >= $2",
        )
        .bind(agent_id)
        .bind(since)
        .fetch_one(pool)
        .await?;
        Ok(row.0)
    }

    pub async fn list_for_agent(
        pool: &PgPool,
        agent_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DbCostRecord>> {
        let rows = sqlx::query_as::<_, DbCostRecord>(
            "SELECT * FROM cost_records \
             WHERE agent_id = $1 \
             ORDER BY created_at DESC \
             LIMIT $2",
        )
        .bind(agent_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

/// Thin repository layer for `DbGovernanceAudit`.
/// // Called by: worker::governance_engine after each policy evaluation.
pub struct GovernanceAuditRepo;

impl GovernanceAuditRepo {
    pub async fn insert(pool: &PgPool, entry: &DbGovernanceAudit) -> Result<()> {
        sqlx::query(
            "INSERT INTO governance_audit \
             (id, agent_id, room_id, orchestration_run_id, triggered_by_user_id, action, result, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(entry.id)
        .bind(entry.agent_id)
        .bind(entry.room_id)
        .bind(entry.orchestration_run_id)
        .bind(&entry.triggered_by_user_id)
        .bind(&entry.action)
        .bind(&entry.result)
        .bind(entry.created_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list_for_agent(
        pool: &PgPool,
        agent_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DbGovernanceAudit>> {
        let rows = sqlx::query_as::<_, DbGovernanceAudit>(
            "SELECT * FROM governance_audit \
             WHERE agent_id = $1 \
             ORDER BY created_at DESC \
             LIMIT $2",
        )
        .bind(agent_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

/// Thin repository layer for `DbDeployment`.
/// // Called by: worker::deploy adapters, gateway::deploy_handlers.
pub struct DeploymentRepo;

impl DeploymentRepo {
    pub async fn insert(pool: &PgPool, deployment: &DbDeployment) -> Result<()> {
        sqlx::query(
            "INSERT INTO deployments \
             (id, provider, config, status, url, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (id) DO UPDATE SET \
               provider = EXCLUDED.provider, \
               config = EXCLUDED.config, \
               status = EXCLUDED.status, \
               url = EXCLUDED.url, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(deployment.id)
        .bind(&deployment.provider)
        .bind(&deployment.config)
        .bind(&deployment.status)
        .bind(&deployment.url)
        .bind(deployment.created_at)
        .bind(deployment.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<DbDeployment>> {
        let row = sqlx::query_as::<_, DbDeployment>("SELECT * FROM deployments WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        Ok(row)
    }

    pub async fn update_status(
        pool: &PgPool,
        id: Uuid,
        status: &str,
        url: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE deployments SET status = $1, url = $2, updated_at = NOW() WHERE id = $3",
        )
        .bind(status)
        .bind(url)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list_all(pool: &PgPool, limit: i64) -> Result<Vec<DbDeployment>> {
        let rows = sqlx::query_as::<_, DbDeployment>(
            "SELECT * FROM deployments ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM deployments WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// Repository for gateway fleet node registry.
pub struct FleetNodeRepo;

impl FleetNodeRepo {
    pub async fn upsert(pool: &PgPool, node: &DbFleetNode) -> Result<()> {
        sqlx::query(
            "INSERT INTO fleet_nodes \
             (id, name, node_type, host, port, status, agent_ids, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (id) DO UPDATE SET \
               name = EXCLUDED.name, \
               node_type = EXCLUDED.node_type, \
               host = EXCLUDED.host, \
               port = EXCLUDED.port, \
               status = EXCLUDED.status, \
               agent_ids = EXCLUDED.agent_ids, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(node.id)
        .bind(&node.name)
        .bind(&node.node_type)
        .bind(&node.host)
        .bind(node.port)
        .bind(&node.status)
        .bind(&node.agent_ids)
        .bind(node.created_at)
        .bind(node.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<DbFleetNode>> {
        let rows = sqlx::query_as::<_, DbFleetNode>(
            "SELECT * FROM fleet_nodes ORDER BY updated_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM fleet_nodes WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// Repository for governance policy CRUD.
pub struct GovernancePolicyRepo;

impl GovernancePolicyRepo {
    pub async fn upsert(pool: &PgPool, policy: &DbGovernancePolicy) -> Result<()> {
        sqlx::query(
            "INSERT INTO governance_policies \
             (id, name, description, rules, enforcement, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (id) DO UPDATE SET \
               name = EXCLUDED.name, \
               description = EXCLUDED.description, \
               rules = EXCLUDED.rules, \
               enforcement = EXCLUDED.enforcement, \
               enabled = EXCLUDED.enabled, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(policy.id)
        .bind(&policy.name)
        .bind(&policy.description)
        .bind(&policy.rules)
        .bind(&policy.enforcement)
        .bind(policy.enabled)
        .bind(policy.created_at)
        .bind(policy.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<DbGovernancePolicy>> {
        let rows = sqlx::query_as::<_, DbGovernancePolicy>(
            "SELECT * FROM governance_policies ORDER BY updated_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM governance_policies WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// Gateway LLM provider registry.
pub struct GatewayProviderRepo;

impl GatewayProviderRepo {
    pub async fn upsert(pool: &PgPool, row: &DbGatewayProvider) -> Result<()> {
        sqlx::query(
            "INSERT INTO gateway_providers \
             (id, name, provider_type, api_key, base_url, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (id) DO UPDATE SET \
               name = EXCLUDED.name, \
               provider_type = EXCLUDED.provider_type, \
               api_key = EXCLUDED.api_key, \
               base_url = EXCLUDED.base_url, \
               enabled = EXCLUDED.enabled, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(row.id)
        .bind(&row.name)
        .bind(&row.provider_type)
        .bind(&row.api_key)
        .bind(&row.base_url)
        .bind(row.enabled)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<DbGatewayProvider>> {
        let rows = sqlx::query_as::<_, DbGatewayProvider>(
            "SELECT * FROM gateway_providers ORDER BY updated_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM gateway_providers WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// Gateway tool registry.
pub struct GatewayToolRepo;

impl GatewayToolRepo {
    pub async fn upsert(pool: &PgPool, row: &DbGatewayTool) -> Result<()> {
        sqlx::query(
            "INSERT INTO gateway_tools \
             (id, name, description, tool_type, config, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (id) DO UPDATE SET \
               name = EXCLUDED.name, \
               description = EXCLUDED.description, \
               tool_type = EXCLUDED.tool_type, \
               config = EXCLUDED.config, \
               enabled = EXCLUDED.enabled, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(row.id)
        .bind(&row.name)
        .bind(&row.description)
        .bind(&row.tool_type)
        .bind(&row.config)
        .bind(row.enabled)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<DbGatewayTool>> {
        let rows = sqlx::query_as::<_, DbGatewayTool>(
            "SELECT * FROM gateway_tools ORDER BY updated_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM gateway_tools WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// Gateway-wide audit trail (no agent FK — all resource types).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGatewayAudit {
    pub id: Uuid,
    pub actor: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub details: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGatewayAudit {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            actor: row.try_get("actor")?,
            action: row.try_get("action")?,
            resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?,
            details: row.try_get("details")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

pub struct GatewayAuditRepo;

impl GatewayAuditRepo {
    pub async fn insert(pool: &PgPool, entry: &DbGatewayAudit) -> Result<()> {
        sqlx::query(
            "INSERT INTO gateway_audit \
             (id, actor, action, resource_type, resource_id, details, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(entry.id)
        .bind(&entry.actor)
        .bind(&entry.action)
        .bind(&entry.resource_type)
        .bind(&entry.resource_id)
        .bind(&entry.details)
        .bind(entry.created_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list_recent(pool: &PgPool, limit: i64) -> Result<Vec<DbGatewayAudit>> {
        let rows = sqlx::query_as::<_, DbGatewayAudit>(
            "SELECT * FROM gateway_audit ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// Filtered, paginated audit rows (newest first) plus total match count.
    pub async fn list_filtered(
        pool: &PgPool,
        resource_type: Option<&str>,
        actor: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<DbGatewayAudit>, i64)> {
        let rows = sqlx::query_as::<_, DbGatewayAudit>(
            "SELECT * FROM gateway_audit \
             WHERE ($1::text IS NULL OR resource_type = $1) \
               AND ($2::text IS NULL OR actor = $2) \
             ORDER BY created_at DESC \
             LIMIT $3 OFFSET $4",
        )
        .bind(resource_type)
        .bind(actor)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM gateway_audit \
             WHERE ($1::text IS NULL OR resource_type = $1) \
               AND ($2::text IS NULL OR actor = $2)",
        )
        .bind(resource_type)
        .bind(actor)
        .fetch_one(pool)
        .await?;

        Ok((rows, total.0))
    }
}

/// Registered gateway user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGatewayUser {
    pub id: Uuid,
    pub tenant_id: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGatewayUser {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            email: row.try_get("email")?,
            password_hash: row.try_get("password_hash")?,
            role: row.try_get("role")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// API key issued to a gateway user (hashed at rest).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGatewayApiKey {
    pub id: Uuid,
    pub tenant_id: String,
    pub user_id: Uuid,
    pub key_hash: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGatewayApiKey {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            user_id: row.try_get("user_id")?,
            key_hash: row.try_get("key_hash")?,
            label: row.try_get("label")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

/// External communication channel registered with the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbGatewayChannel {
    pub id: Uuid,
    pub tenant_id: String,
    pub name: String,
    pub channel_type: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGatewayChannel {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            name: row.try_get("name")?,
            channel_type: row.try_get("channel_type")?,
            config: row.try_get("config")?,
            enabled: row.try_get("enabled")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

pub struct GatewayUserRepo;

impl GatewayUserRepo {
    pub async fn upsert(pool: &PgPool, row: &DbGatewayUser) -> Result<()> {
        sqlx::query(
            "INSERT INTO gateway_users \
             (id, tenant_id, email, password_hash, role, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (id) DO UPDATE SET \
               email = EXCLUDED.email, \
               password_hash = EXCLUDED.password_hash, \
               role = EXCLUDED.role, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(row.id)
        .bind(&row.tenant_id)
        .bind(&row.email)
        .bind(&row.password_hash)
        .bind(&row.role)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_email(
        pool: &PgPool,
        tenant_id: &str,
        email: &str,
    ) -> Result<Option<DbGatewayUser>> {
        let row = sqlx::query_as::<_, DbGatewayUser>(
            "SELECT * FROM gateway_users WHERE tenant_id = $1 AND email = $2",
        )
        .bind(tenant_id)
        .bind(email)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<DbGatewayUser>> {
        let rows = sqlx::query_as::<_, DbGatewayUser>(
            "SELECT * FROM gateway_users ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

pub struct GatewayApiKeyRepo;

impl GatewayApiKeyRepo {
    pub async fn insert(pool: &PgPool, row: &DbGatewayApiKey) -> Result<()> {
        sqlx::query(
            "INSERT INTO gateway_api_keys \
             (id, tenant_id, user_id, key_hash, label, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(row.id)
        .bind(&row.tenant_id)
        .bind(row.user_id)
        .bind(&row.key_hash)
        .bind(&row.label)
        .bind(row.created_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<DbGatewayApiKey>> {
        let rows = sqlx::query_as::<_, DbGatewayApiKey>(
            "SELECT * FROM gateway_api_keys ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}

pub struct GatewayChannelRepo;

impl GatewayChannelRepo {
    pub async fn upsert(pool: &PgPool, row: &DbGatewayChannel) -> Result<()> {
        sqlx::query(
            "INSERT INTO gateway_channels \
             (id, tenant_id, name, channel_type, config, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (id) DO UPDATE SET \
               name = EXCLUDED.name, \
               channel_type = EXCLUDED.channel_type, \
               config = EXCLUDED.config, \
               enabled = EXCLUDED.enabled, \
               updated_at = EXCLUDED.updated_at",
        )
        .bind(row.id)
        .bind(&row.tenant_id)
        .bind(&row.name)
        .bind(&row.channel_type)
        .bind(&row.config)
        .bind(row.enabled)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<DbGatewayChannel>> {
        let rows = sqlx::query_as::<_, DbGatewayChannel>(
            "SELECT * FROM gateway_channels ORDER BY updated_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_for_tenant(
        pool: &PgPool,
        tenant_id: &str,
        limit: i64,
    ) -> Result<Vec<DbGatewayChannel>> {
        let rows = sqlx::query_as::<_, DbGatewayChannel>(
            "SELECT * FROM gateway_channels \
             WHERE tenant_id = $1 \
             ORDER BY updated_at DESC \
             LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM gateway_channels WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}
