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
pub mod bootstrap;
pub mod cloudflare;
pub mod connectors;
pub mod db_bootstrap;
pub mod deploy;
pub mod mcp;
pub mod password;
pub mod postgres_platform_store;
pub mod postgres_store;
pub mod prism_check;
pub mod routes;
pub mod tool_catalog;
pub mod scheduling;
pub mod secrets;
pub mod server;
pub mod shutdown;
pub mod telephony;
pub mod tui;
pub mod voice_pipeline;
pub mod worker_fleet;
pub mod ws;

use axum::{http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Utc};
use clawz_core::traits::AgentScheduler;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

// ─── Domain record types ──────────────────────────────────────────────────────

/// Runtime lifecycle states for an agent instance.
///
/// Stored as lowercase strings when serialized via serde.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// Agent is idle and ready to accept work.
    #[default]
    Idle,
    /// Agent is currently executing a task.
    Running,
    /// Agent was explicitly stopped and is not accepting work.
    Stopped,
    /// Agent encountered an unrecoverable error during execution.
    Error,
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

/// Participant in a multi-participant agent room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomParticipantRecord {
    /// User or agent identifier.
    pub participant_id: String,
    /// `"user"` or `"agent"`.
    pub participant_type: String,
    /// Room role: `"owner"`, `"member"`, `"observer"`, etc.
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

/// Message within a multi-participant room (sequenced, visibility-aware).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMessageRecord {
    pub id: String,
    pub room_id: String,
    /// Monotonic sequence number within the room.
    pub seq: u64,
    /// `"user"`, `"agent"`, or `"system"`.
    #[serde(default = "default_sender_type_user")]
    pub sender_type: String,
    pub sender_id: String,
    pub client_message_id: Option<String>,
    pub content: String,
    pub mentions: Vec<String>,
    /// `"room"`, `"side_thread"`, or `"private"`.
    pub visibility: String,
    /// Set when the message belongs to a side-thread (Hybrid C).
    pub thread_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

fn default_sender_type_user() -> String {
    "user".to_string()
}

/// Private side-thread scoped to a subset of room participants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomSideThreadRecord {
    pub id: String,
    pub room_id: String,
    pub title: Option<String>,
    pub participant_ids: Vec<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

/// Multi-participant agent room with orchestration binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomRecord {
    pub id: String,
    pub tenant_id: String,
    pub room_type: String,
    pub orchestration_mode: Option<String>,
    pub orchestration_config: Option<serde_json::Value>,
    pub participants: Vec<RoomParticipantRecord>,
    pub messages: Vec<RoomMessageRecord>,
    pub side_threads: Vec<RoomSideThreadRecord>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

fn default_channel_tenant() -> String {
    "default".to_string()
}

/// An external communication channel integrated into the platform.
///
/// Channels allow agents to receive inputs and send outputs via Slack, email,
/// generic webhooks, and other third-party services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    /// Unique identifier for this channel integration.
    pub id: String,
    /// Tenant that owns this channel integration.
    #[serde(default = "default_channel_tenant")]
    pub tenant_id: String,
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
/// Dashboard-editable gateway settings (in-memory until a config store exists).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UiSettings {
    pub log_level: String,
    pub max_agents: u32,
    pub enable_audit: bool,
}

impl UiSettings {
    pub fn from_env() -> Self {
        let max_agents = std::env::var("CLAWZ_MAX_AGENTS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);
        let enable_audit = std::env::var("CLAWZ_ENABLE_AUDIT")
            .ok()
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        Self {
            log_level: std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
            max_agents,
            enable_audit,
        }
    }
}

/// The current implementation uses in-memory vectors for rapid prototyping.
/// In production this will likely be backed by PostgreSQL or Redis.
#[derive(Clone)]
pub struct AppState {
    /// In-memory registry of all agent definitions.
    pub agents: Arc<RwLock<Vec<AgentRecord>>>,
    /// In-memory registry of all conversations.
    pub conversations: Arc<RwLock<Vec<ConversationRecord>>>,
    /// In-memory registry of multi-participant agent rooms.
    pub rooms: Arc<RwLock<Vec<RoomRecord>>>,
    /// Active orchestration run IDs keyed by room ID.
    pub room_runs: Arc<RwLock<HashMap<String, String>>>,
    /// Room IDs with an agent turn currently in flight.
    pub room_turn_inflight: Arc<RwLock<HashSet<String>>>,
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
    /// Per-room broadcast channels for multi-participant agent room WebSockets.
    pub room_broadcasts: Arc<RwLock<HashMap<String, broadcast::Sender<String>>>>,
    /// Secret key used to sign and verify JWT access tokens.
    pub jwt_secret: String,
    /// UTC timestamp recorded when the gateway process started.
    ///
    /// Used to compute uptime in health-check responses.
    pub start_time: DateTime<Utc>,
    /// Worker delegation layer (in-process or HTTP).
    pub platform: Option<Arc<clawz_services::Platform>>,
    /// Optional Postgres pool (migrations applied at startup).
    pub db: Option<sqlx::PgPool>,
    /// Optional Postgres-backed platform store (agents CRUD).
    pub platform_store: Option<Arc<dyn clawz_services::PlatformStore>>,
    /// Fleet deploy scheduler (standalone or Docker per `CLAWZ_MODE`).
    pub agent_scheduler: Option<Arc<dyn AgentScheduler>>,
    /// Per-tenant admission before fleet spawn.
    pub admission: Arc<crate::scheduling::AdmissionController>,
    /// Routes to warm agents or spawns via in-process scheduler (standalone).
    pub tenant_router: Option<Arc<crate::scheduling::TenantRouter>>,
    /// Delegates fleet spawn/list to worker control API when `WORKER_URL` is set.
    pub worker_fleet: Option<Arc<crate::worker_fleet::WorkerFleetClient>>,
    /// Cloud provider deployment orchestrator (18 adapters).
    pub deploy_manager: Arc<crate::deploy::DeployManager>,
    /// Per-agent personality metadata (traits, tone) until persisted in identity store.
    pub personality: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// Human-in-the-loop approval queue (shared with worker in standalone mode).
    pub approval_workflow: Arc<clawz_worker::governance::approval::ApprovalWorkflow>,
    /// Runtime UI settings (survives until gateway restart).
    pub ui_settings: Arc<RwLock<UiSettings>>,
}

/// Personality preferences stored per agent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersonalityPrefs {
    pub traits: Vec<String>,
    pub tone: String,
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
        let approval = Arc::new(clawz_worker::governance::approval::ApprovalWorkflow::new());
        Self::full(jwt_secret, identity_store, None, None, approval, None, None)
    }

    pub fn full(
        jwt_secret: impl Into<String>,
        identity_store: Option<Arc<clawz_worker::runtime::identity::AgentIdentityStore>>,
        platform: Option<Arc<clawz_services::Platform>>,
        db: Option<sqlx::PgPool>,
        approval_workflow: Arc<clawz_worker::governance::approval::ApprovalWorkflow>,
        platform_store: Option<Arc<dyn clawz_services::PlatformStore>>,
        agent_scheduler: Option<Arc<dyn AgentScheduler>>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let db_for_deploy = db.clone();
        let admission = Arc::new(crate::scheduling::AdmissionController::new(
            std::env::var("CLAWZ_MAX_CONCURRENT_PER_TENANT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            100,
        ));
        let tenant_router = agent_scheduler
            .as_ref()
            .map(|s| Arc::new(crate::scheduling::TenantRouter::new(s.clone())));
        let worker_fleet = crate::worker_fleet::WorkerFleetClient::from_env().map(Arc::new);
        Self {
            agents: Arc::new(RwLock::new(Vec::new())),
            conversations: Arc::new(RwLock::new(Vec::new())),
            rooms: Arc::new(RwLock::new(Vec::new())),
            room_runs: Arc::new(RwLock::new(HashMap::new())),
            room_turn_inflight: Arc::new(RwLock::new(HashSet::new())),
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
            room_broadcasts: Arc::new(RwLock::new(HashMap::new())),
            jwt_secret: jwt_secret.into(),
            start_time: Utc::now(),
            platform,
            db,
            platform_store,
            agent_scheduler,
            admission,
            tenant_router,
            worker_fleet,
            deploy_manager: Arc::new(crate::deploy::DeployManager::new_default(
                Arc::new(crate::deploy::MemoryDeploymentStore::new()),
                db_for_deploy,
            )),
            personality: Arc::new(RwLock::new(HashMap::new())),
            approval_workflow,
            ui_settings: Arc::new(RwLock::new(UiSettings::from_env())),
        }
    }

    /// Subscribe to platform events when configured, otherwise the legacy broadcast channel.
    pub fn subscribe_events(&self) -> broadcast::Receiver<String> {
        if let Some(ref platform) = self.platform {
            platform.events.subscribe()
        } else {
            self.event_tx.subscribe()
        }
    }

    /// Publish an event to all WebSocket subscribers.
    pub fn publish_event(&self, event_type: &str, payload: serde_json::Value) {
        if let Some(ref platform) = self.platform {
            platform.events.publish_json(event_type, payload);
        } else {
            let envelope = serde_json::json!({
                "type": event_type,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "data": payload,
            });
            let _ = self.event_tx.send(envelope.to_string());
        }
    }

    /// Subscribe to the broadcast channel for a multi-participant room.
    ///
    /// Creates a channel on first subscription so later publishers can reach
    /// connected clients.
    pub async fn subscribe_room(&self, room_id: &str) -> broadcast::Receiver<String> {
        let mut map = self.room_broadcasts.write().await;
        if let Some(tx) = map.get(room_id) {
            tx.subscribe()
        } else {
            let (tx, rx) = broadcast::channel(256);
            map.insert(room_id.to_string(), tx);
            rx
        }
    }

    /// Publish a JSON event to all WebSocket subscribers in a room.
    pub async fn publish_room_event(&self, room_id: &str, json: serde_json::Value) {
        let tx = {
            let mut map = self.room_broadcasts.write().await;
            map.entry(room_id.to_string())
                .or_insert_with(|| {
                    let (tx, _) = broadcast::channel(256);
                    tx
                })
                .clone()
        };
        let _ = tx.send(json.to_string());
    }
}

impl Default for AppState {
    fn default() -> Self {
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
        self.audit_log.write().await.push(entry.clone());
        if let Some(ref pool) = self.db {
            crate::postgres_store::persist_audit(pool, &entry).await;
        }
    }

    /// Best-effort Postgres persistence for an agent record.
    pub async fn persist_agent_record(&self, record: &AgentRecord) {
        if let Some(ref pool) = self.db {
            let _ = crate::postgres_store::persist_agent(pool, record).await;
        }
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
    /// Request conflicts with current resource state (e.g. duplicate in-flight turn).
    Conflict(String),
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
            GatewayError::Conflict(msg) => write!(f, "conflict: {msg}"),
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
            GatewayError::Unauthorized(msg) => {
                (StatusCode::UNAUTHORIZED, "unauthorized", msg.clone())
            }
            GatewayError::Conflict(msg) => (StatusCode::CONFLICT, "conflict", msg.clone()),
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
        (
            status,
            Json(json!({"error": error_code, "message": message})),
        )
            .into_response()
    }
}

/// Shorthand result type used by gateway route handlers.
pub type GatewayResult<T> = Result<T, GatewayError>;
