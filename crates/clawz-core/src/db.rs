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

/// Individual message within a conversation.
/// // Dependency: mapped to table `messages` by MessageRepo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbMessage {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbMessage {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            conversation_id: row.try_get("conversation_id")?,
            role: row.try_get("role")?,
            content: row.try_get("content")?,
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
    pub action: String,
    pub result: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for DbGovernanceAudit {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            agent_id: row.try_get("agent_id")?,
            action: row.try_get("action")?,
            result: row.try_get("result")?,
            created_at: row.try_get("created_at")?,
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
"#;

// ── run_migrations ────────────────────────────────────────────────────────────

/// Execute all DDL migrations against the connected pool.
/// This is idempotent (uses `IF NOT EXISTS` everywhere).
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    for statement in MIGRATIONS_SQL.split(';') {
        let trimmed = statement.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
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
        let rows =
            sqlx::query_as::<_, DbAgent>(
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
        sqlx::query(
            "INSERT INTO conversations \
             (id, agent_id, channel_id, started_at, last_message_at, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6)",
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
        let row =
            sqlx::query_as::<_, DbConversation>("SELECT * FROM conversations WHERE id = $1")
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
}

/// Thin repository layer for `DbMessage`.
/// // Called by: worker::conversation_manager, worker::memory backends
pub struct MessageRepo;

impl MessageRepo {
    pub async fn insert(pool: &PgPool, msg: &DbMessage) -> Result<()> {
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, role, content, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(msg.id)
        .bind(msg.conversation_id)
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(msg.created_at)
        .execute(pool)
        .await?;
        Ok(())
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
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::BIGINT FROM messages WHERE conversation_id = $1",
        )
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
             (id, agent_id, provider, model, input_tokens, output_tokens, cost_usd, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(record.id)
        .bind(record.agent_id)
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
    pub async fn total_since(
        pool: &PgPool,
        agent_id: Uuid,
        since: DateTime<Utc>,
    ) -> Result<f64> {
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
            "INSERT INTO governance_audit (id, agent_id, action, result, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(entry.id)
        .bind(entry.agent_id)
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
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
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
        let row =
            sqlx::query_as::<_, DbDeployment>("SELECT * FROM deployments WHERE id = $1")
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
}
