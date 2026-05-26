//! Clawz Gateway — HTTP API entry point for the Clawz agent orchestration platform.
//!
//! This crate forms the **API layer** of a 3-tier architecture:
//! `gateway` (API layer) → `worker` (execution layer) → `core` (shared types/traits).
//!
//! Responsibilities:
//! - Accept and authenticate incoming HTTP/WebSocket requests.
//! - Route requests to the appropriate domain handlers (agents, conversations, fleet, etc.).
//! - Maintain in-memory domain state (agents, channels, providers, fleet nodes, etc.).
//! - Emit audit entries and broadcast events to WebSocket subscribers.
//! - Schedule and deploy agent workloads onto worker nodes via the fleet subsystem.
//!
//! Key modules:
//! - [`server`] — Axum HTTP server setup and middleware stack.
//! - [`shutdown`] — Graceful shutdown coordination via [`tokio_util::sync::CancellationToken`].
//! - [`routes`] — REST API route handlers for all domain resources.
//! - [`ws`] — WebSocket handlers for streaming agent output, events, metrics, and logs.
//! - [`auth`] — JWT-based authentication and API key validation.
//! - [`scheduling`] — Agent workload placement and scheduling logic.
//!
//! # Feature flags
//! This crate is designed to compile as a standalone binary or as a library for integration tests.

pub mod auth;
pub mod cloudflare;
pub mod connectors;
pub mod deploy;
pub mod mcp;
pub mod routes;
pub mod scheduling;
pub mod server;
pub mod shutdown;
pub mod tui;
pub mod ws;

use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use axum::{http::StatusCode, response::IntoResponse};

// ─── Domain record types ──────────────────────────────────────────────────────

/// Runtime lifecycle states for an agent instance.
///
/// Stored as lowercase strings when serialized via serde.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// Agent is idle and ready to accept work.
    Idle,
    /// Agent is currently executing a task.
    Running,
    /// Agent was explicitly stopped and is not accepting work.
    Stopped,
    /// Agent encountered an unrecoverable error during execution.
    Error,
}

impl Default for AgentStatus {
    fn default() -> Self { AgentStatus::Idle }
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Idle => write!(f, "idle"),
            AgentStatus::Running => write!(f, "running"),
            AgentStatus::Stopped => write!(f, "stopped"),
            AgentStatus::Error => write!(f, "error"),
        }
    }
}

/// Persistent record for an AI agent definition.
///
/// Agents are the central unit of execution in Clawz. Each agent binds to a
/// specific model provider and carries a system prompt that shapes its behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    /// Unique identifier (UUID v4) for this agent.
    pub id: String,
    /// Human-readable display name chosen by the user.
    pub name: String,
    /// Identifier of the LLM model to use (e.g., "gpt-4", "claude-3-opus").
    pub model: String,
    /// Optional long-form description of the agent's purpose.
    pub description: Option<String>,
    /// Optional system prompt injected at the start of every conversation.
    pub system_prompt: Option<String>,
    /// Current runtime status (idle, running, stopped, error).
    pub status: AgentStatus,
    /// Timestamp when this record was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent update to this record.
    pub updated_at: DateTime<Utc>,
}

/// A single message within a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    /// Unique identifier for this message.
    pub id: String,
    /// Foreign key referencing the parent [`ConversationRecord`].
    pub conversation_id: String,
    /// Role of the message sender: `"user"`, `"assistant"`, or `"system"`.
    pub role: String,
    /// Raw text content of the message.
    pub content: String,
    /// Timestamp when the message was received or generated.
    pub created_at: DateTime<Utc>,
}

/// A threaded conversation between a user and an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    /// Unique identifier for this conversation.
    pub id: String,
    /// Foreign key referencing the [`AgentRecord`] that participates in this conversation.
    pub agent_id: String,
    /// Optional user-defined title (auto-generated if absent).
    pub title: Option<String>,
    /// Whether this conversation has been soft-deleted / archived.
    pub archived: bool,
    /// Ordered list of all messages in the conversation.
    pub messages: Vec<MessageRecord>,
    /// Timestamp when the conversation was started.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent message or metadata update.
    pub updated_at: DateTime<Utc>,
}

/// An external communication channel integrated into the platform.
///
/// Channels allow agents to receive inputs and send outputs via Slack, email,
/// generic webhooks, and other third-party services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    /// Unique identifier for this channel integration.
    pub id: String,
    /// Human-readable name shown in the UI.
    pub name: String,
    /// Channel category: `"slack"`, `"email"`, `"webhook"`, etc.
    pub channel_type: String,
    /// Opaque JSON blob holding provider-specific configuration (webhook URLs, tokens, etc.).
    pub config: serde_json::Value,
    /// Whether this channel is active and should receive events.
    pub enabled: bool,
    /// Timestamp when the channel was registered.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent configuration update.
    pub updated_at: DateTime<Utc>,
}

/// A configured LLM provider (e.g., OpenAI, Anthropic, AWS Bedrock).
///
/// Providers abstract the underlying API credentials and base URLs so that
/// agents can be moved between providers without changing their model identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRecord {
    /// Unique identifier for this provider configuration.
    pub id: String,
    /// Human-readable label (e.g., "Production OpenAI").
    pub name: String,
    /// Provider kind: `"openai"`, `"anthropic"`, `"bedrock"`, etc.
    pub provider_type: String,
    /// Optional API key stored in plain text (for prototyping; use a secrets manager in production).
    pub api_key: Option<String>,
    /// Optional custom base URL for self-hosted or proxy endpoints.
    pub base_url: Option<String>,
    /// Whether this provider is available for new agent allocations.
    pub enabled: bool,
    /// Timestamp when the provider was registered.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent configuration update.
    pub updated_at: DateTime<Utc>,
}

/// A tool that agents can invoke during execution.
///
/// Tools extend agent capabilities beyond text generation, enabling function
/// calling, external API invocation, and MCP (Model Context Protocol) integrations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRecord {
    /// Unique identifier for this tool.
    pub id: String,
    /// Short name used in tool-call syntax.
    pub name: String,
    /// Description shown to the LLM when deciding which tool to invoke.
    pub description: String,
    /// Tool category: `"function"`, `"api"`, `"mcp"`, etc.
    pub tool_type: String,
    /// Opaque JSON blob holding tool-specific configuration (schema, endpoint, parameters).
    pub config: serde_json::Value,
    /// Whether this tool is currently available to agents.
    pub enabled: bool,
    /// Timestamp when the tool was registered.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent configuration update.
    pub updated_at: DateTime<Utc>,
}

/// A governance policy that constrains agent behavior.
///
/// Policies define guardrails such as output filtering, rate limits, and
/// forbidden topic restrictions. Each policy carries a list of rule strings
/// and an enforcement level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRecord {
    /// Unique identifier for this policy.
    pub id: String,
    /// Human-readable name shown in the admin dashboard.
    pub name: String,
    /// Long-form explanation of what this policy guards against.
    pub description: String,
    /// List of rule expressions or topic strings evaluated by the policy engine.
    pub rules: Vec<String>,
    /// Enforcement severity: `"block"` (reject), `"warn"` (log but allow), or `"audit"` (log only).
    pub enforcement: String,
    /// Whether this policy is actively evaluated during agent execution.
    pub enabled: bool,
    /// Timestamp when the policy was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent update.
    pub updated_at: DateTime<Utc>,
}

/// A node in the Clawz fleet (worker, gateway, or coordinator instance).
///
/// Fleet nodes represent the compute topology. The gateway uses this registry
/// to decide where to schedule new agent deployments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetNodeRecord {
    /// Unique node identifier.
    pub id: String,
    /// Human-readable node label.
    pub name: String,
    /// Node role: `"worker"`, `"gateway"`, or `"coordinator"`.
    pub node_type: String,
    /// IP address or DNS hostname for reaching this node.
    pub host: String,
    /// TCP port exposed by the node's HTTP server.
    pub port: u16,
    /// Health status: `"online"`, `"offline"`, or `"degraded"`.
    pub status: String,
    /// IDs of agents currently running on this node.
    pub agent_ids: Vec<String>,
    /// Timestamp when the node first joined the fleet.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the last heartbeat or metadata update.
    pub updated_at: DateTime<Utc>,
}

/// Tracks the deployment of an agent onto a specific fleet node.
///
/// Deployments form a many-to-many link between [`AgentRecord`] and [`FleetNodeRecord`],
/// capturing the lifecycle of an agent workload on the execution layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentRecord {
    /// Unique identifier for this deployment.
    pub id: String,
    /// Foreign key referencing the deployed [`AgentRecord`].
    pub agent_id: String,
    /// Foreign key referencing the target [`FleetNodeRecord`].
    pub node_id: String,
    /// Deployment phase: `"pending"`, `"running"`, `"failed"`, or `"complete"`.
    pub status: String,
    /// Timestamp when the deployment was initiated.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent status transition.
    pub updated_at: DateTime<Utc>,
}

/// A single entry in the platform-wide audit log.
///
/// Audit entries capture who did what, to which resource, and when.
/// They are immutable and append-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique identifier for this audit entry.
    pub id: String,
    /// Identity of the actor (user email, service name, or "system").
    pub actor: String,
    /// The operation performed (e.g., "agent.create", "deployment.start").
    pub action: String,
    /// Domain category of the affected resource.
    pub resource_type: String,
    /// Unique identifier of the affected resource.
    pub resource_id: String,
    /// Optional free-form JSON or text with additional context.
    pub details: Option<String>,
    /// Timestamp when the action occurred.
    pub created_at: DateTime<Utc>,
}

/// A hashed API key belonging to a registered user.
///
/// API keys are used for service-to-service and programmatic authentication.
/// Only the hash is stored; the raw key is returned once at creation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    /// Unique identifier for this API key.
    pub id: String,
    /// Foreign key referencing the owning [`UserRecord`].
    pub user_id: String,
    /// bcrypt or similar hash of the raw API key string.
    pub key_hash: String,
    /// User-defined label to distinguish multiple keys.
    pub label: String,
    /// Timestamp when the key was generated.
    pub created_at: DateTime<Utc>,
}

/// A registered human user of the Clawz platform.
///
/// Users authenticate via email/password (JWT) or API keys. The `role` field
/// controls access to admin-level endpoints such as fleet management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    /// Unique identifier for this user.
    pub id: String,
    /// Email address used for login and notifications.
    pub email: String,
    /// Hashed password (argon2 or bcrypt).
    pub password_hash: String,
    /// Access level: `"admin"` or `"user"`.
    pub role: String,
    /// Timestamp when the account was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent profile update.
    pub updated_at: DateTime<Utc>,
}

/// Lifecycle states for an [`AutonomousSessionRecord`].
///
/// Mirrors `clawz_worker::runtime::agent::AutonomousSessionStatus` but lives
/// here to keep the gateway free of a worker crate dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomousSessionStatus {
    /// Session is actively executing turns.
    Running,
    /// Session reached its `max_turns` ceiling.
    MaxTurnsReached,
    /// Session reached its `cost_budget_usd` ceiling.
    BudgetExhausted,
    /// Session completed normally (model emitted a stop signal).
    Completed,
    /// Session was cancelled by a caller.
    Cancelled,
}

/// Tracking record for a long-running autonomous multi-turn session.
///
/// Created by the `POST /agents/{id}/autonomous` endpoint. The gateway stores
/// the bounded budgets and current progress so consumers can read terminal
/// state and the WebSocket layer can publish per-turn activity events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousSessionRecord {
    /// Stable session identifier (UUID-v4).
    pub id: String,
    /// Owning agent ID.
    pub agent_id: String,
    /// Hard upper bound on conversation turns for this session.
    pub max_turns: usize,
    /// Hard upper bound on accumulated USD spend for this session.
    pub cost_budget_usd: f64,
    /// Number of turns executed so far.
    pub turns_executed: usize,
    /// Cumulative USD spent across all turns.
    pub cost_accumulated_usd: f64,
    /// Current lifecycle status.
    pub status: AutonomousSessionStatus,
    /// Optional system-prompt override for this session.
    pub system_prompt: Option<String>,
    /// Timestamp when the session was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent state change.
    pub updated_at: DateTime<Utc>,
}

// ─── AppState ─────────────────────────────────────────────────────────────────

/// Shared application state accessible to all Axum request handlers.
///
/// [`AppState`] is constructed once at startup, wrapped in [`Arc`] + [`axum::extract::State`],
/// and cloned cheaply into every route. Each domain collection is protected by a
/// [`tokio::sync::RwLock`] so that concurrent handlers can read and write safely.
///
/// # Design note
/// The current implementation uses in-memory vectors for rapid prototyping.
/// In production this will likely be backed by PostgreSQL or Redis.
#[derive(Clone)]
pub struct AppState {
    /// In-memory registry of all agent definitions.
    pub agents: Arc<RwLock<Vec<AgentRecord>>>,
    /// In-memory registry of all conversations.
    pub conversations: Arc<RwLock<Vec<ConversationRecord>>>,
    /// In-memory registry of external channel integrations.
    pub channels: Arc<RwLock<Vec<ChannelRecord>>>,
    /// In-memory registry of configured LLM providers.
    pub providers: Arc<RwLock<Vec<ProviderRecord>>>,
    /// In-memory registry of available tools.
    pub tools: Arc<RwLock<Vec<ToolRecord>>>,
    /// In-memory registry of governance policies.
    pub policies: Arc<RwLock<Vec<PolicyRecord>>>,
    /// In-memory registry of fleet compute nodes.
    pub fleet_nodes: Arc<RwLock<Vec<FleetNodeRecord>>>,
    /// In-memory registry of active agent deployments.
    pub deployments: Arc<RwLock<Vec<DeploymentRecord>>>,
    /// In-memory registry of active autonomous multi-turn sessions.
    pub autonomous_sessions: Arc<RwLock<Vec<AutonomousSessionRecord>>>,
    /// Append-only audit log.
    pub audit_log: Arc<RwLock<Vec<AuditEntry>>>,
    /// Registry of hashed API keys for programmatic access.
    pub api_keys: Arc<RwLock<Vec<ApiKeyRecord>>>,
    /// Registry of human user accounts.
    pub users: Arc<RwLock<Vec<UserRecord>>>,
    /// Optional cross-session identity store. When configured, exposes
    /// the identity-version-hash Admin API at `GET /agents/{id}/identity/hash`.
    pub identity_store: Option<Arc<clawz_worker::runtime::identity::AgentIdentityStore>>,
    /// Broadcast channel for real-time events (consumed by WebSocket subscribers).
    ///
    /// Capacity is fixed at 256 because events are best-effort; slow consumers
    /// are dropped rather than blocking the publisher.
    pub event_tx: broadcast::Sender<String>,
    /// Secret key used to sign and verify JWT access tokens.
    pub jwt_secret: String,
    /// UTC timestamp recorded when the gateway process started.
    ///
    /// Used to compute uptime in health-check responses.
    pub start_time: DateTime<Utc>,
}

impl AppState {
    /// Create a new [`AppState`] with empty registries and the given JWT secret.
    ///
    /// The broadcast channel is created with a 256-slot buffer.
    pub fn new(jwt_secret: impl Into<String>) -> Self {
        Self::with_identity_store(jwt_secret, None)
    }

    pub fn with_identity_store(
        jwt_secret: impl Into<String>,
        identity_store: Option<Arc<clawz_worker::runtime::identity::AgentIdentityStore>>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            agents: Arc::new(RwLock::new(Vec::new())),
            conversations: Arc::new(RwLock::new(Vec::new())),
            channels: Arc::new(RwLock::new(Vec::new())),
            providers: Arc::new(RwLock::new(Vec::new())),
            tools: Arc::new(RwLock::new(Vec::new())),
            policies: Arc::new(RwLock::new(Vec::new())),
            fleet_nodes: Arc::new(RwLock::new(Vec::new())),
            deployments: Arc::new(RwLock::new(Vec::new())),
            autonomous_sessions: Arc::new(RwLock::new(Vec::new())),
            audit_log: Arc::new(RwLock::new(Vec::new())),
            api_keys: Arc::new(RwLock::new(Vec::new())),
            users: Arc::new(RwLock::new(Vec::new())),
            identity_store,
            event_tx,
            jwt_secret: jwt_secret.into(),
            start_time: Utc::now(),
        }
    }

    /// Convenience constructor that reads `JWT_SECRET` from the environment.
    ///
    /// Falls back to a hard-coded development secret (`"clawz-secret"`) when the
    /// variable is absent. **Do not use the default in production.**
    pub fn default() -> Self {
        Self::new(std::env::var("JWT_SECRET").unwrap_or_else(|_| "clawz-secret".into()))
    }
}

impl AppState {
    /// Append a new [`AuditEntry`] to the audit log.
    ///
    /// This helper constructs the entry, assigns a fresh UUID, timestamps it, and
    /// pushes it into the shared `audit_log` vector under a write lock. Callers
    /// should fire-and-forget via `spawn` if they do not need to await completion.
    ///
    /// # Arguments
    /// - `actor` — identity string for the performer of the action.
    /// - `action` — namespaced verb such as `"agent.create"` or `"deployment.start"`.
    /// - `resource_type` — domain category of the affected resource.
    /// - `resource_id` — unique identifier of the affected resource.
    /// - `details` — optional extra context (JSON blob or human-readable text).
    pub async fn append_audit(
        &self,
        actor: impl Into<String>,
        action: impl Into<String>,
        resource_type: impl Into<String>,
        resource_id: impl Into<String>,
        details: Option<String>,
    ) {
        let entry = AuditEntry {
            id: Uuid::new_v4().to_string(),
            actor: actor.into(),
            action: action.into(),
            resource_type: resource_type.into(),
            resource_id: resource_id.into(),
            details,
            created_at: Utc::now(),
        };
        self.audit_log.write().await.push(entry);
    }
}

// ─── Health response ──────────────────────────────────────────────────────────

/// Standard health-check payload returned by the `/health` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Overall service status. Expected values: `"ok"`, `"degraded"`, `"down"`.
    pub status: String,
}

// ─── Gateway error ────────────────────────────────────────────────────────────

/// The canonical error type for the gateway crate.
///
/// Every fallible route handler should return [`GatewayResult<T>`]. The
/// [`axum::response::IntoResponse`] implementation maps each variant to an
/// appropriate HTTP status code and a JSON error body shaped as:
/// ```json
/// { "error": "not_found", "message": "agent abc-123 not found" }
/// ```
#[derive(Debug)]
pub enum GatewayError {
    /// The requested resource (by type and ID) does not exist.
    NotFound { resource: String, id: String },
    /// The request body or parameters failed semantic validation.
    Unprocessable(String),
    /// The caller did not supply valid credentials or lacks permission.
    Unauthorized(String),
    /// An unexpected internal failure (database, network, worker RPC, etc.).
    Internal(String),
    /// The endpoint exists but the underlying feature is not yet implemented.
    NotImplemented,
}

impl GatewayError {
    /// Convenience constructor for [`GatewayError::NotFound`] with string slices.
    pub fn not_found(resource: &str, id: &str) -> Self {
        GatewayError::NotFound {
            resource: resource.to_string(),
            id: id.to_string(),
        }
    }
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayError::NotFound { resource, id } => write!(f, "{resource} {id} not found"),
            GatewayError::Unprocessable(msg) => write!(f, "unprocessable: {msg}"),
            GatewayError::Unauthorized(msg) => write!(f, "unauthorized: {msg}"),
            GatewayError::Internal(msg) => write!(f, "internal error: {msg}"),
            GatewayError::NotImplemented => write!(f, "not implemented"),
        }
    }
}

impl std::error::Error for GatewayError {}

impl IntoResponse for GatewayError {
    fn into_response(self) -> axum::response::Response {
        use axum::Json;
        use serde_json::json;
        // Map each error variant to an HTTP status code, a machine-readable error code,
        // and a human-readable message. This keeps route handlers declarative: they
        // simply return `Err(GatewayError::*)` and the middleware handles serialization.
        let (status, error_code, message) = match &self {
            GatewayError::NotFound { resource, id } => (
                StatusCode::NOT_FOUND,
                "not_found",
                format!("{resource} {id} not found"),
            ),
            GatewayError::Unprocessable(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "unprocessable_entity",
                msg.clone(),
            ),
            GatewayError::Unauthorized(msg) => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                msg.clone(),
            ),
            GatewayError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                msg.clone(),
            ),
            GatewayError::NotImplemented => (
                StatusCode::NOT_IMPLEMENTED,
                "not_implemented",
                "not implemented".to_string(),
            ),
        };
        (status, Json(json!({"error": error_code, "message": message}))).into_response()
    }
}

/// Shorthand result type used by gateway route handlers.
pub type GatewayResult<T> = Result<T, GatewayError>;
