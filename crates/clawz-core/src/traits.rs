//! Async trait interfaces for every pluggable subsystem in ClawZ.
//!
//! This module defines the 12 core trait contracts. Each trait is
//! implemented in `clawz-worker` by a concrete adapter and then
//! wired into the runtime via dependency injection.
//!
//! Trait list:
//!   1. `Provider`        — LLM adapter (OpenAI, Anthropic, …)
//!   2. `PipelineStep`    — Mutable processing step in the agent pipeline
//!   3. `ChannelPlugin`   — Slack, Discord, Telegram, WhatsApp, email, …
//!   4. `Tool`            — Executable capability exposed to agents
//!   5. `MemoryBackend`   — Vector + key-value storage for agent memory
//!   6. `Transport`       — Mesh peer-to-peer network abstraction
//!   7. `GovernanceEngine` — Policy evaluation and trust scoring
//!   8. `DeployAdapter`   — Docker, Fly, Railway, k8s deployment
//!   9. `SaaSConnector`   — OAuth2/API-key SaaS integrations
//!  10. `AgentScheduler`  — Spawn / reap / health-check agent containers
//!  11. `ToolOrchestrator` — Spawn / reap / health-check tool containers
//!  12. `TenantMesh`      — Per-tenant overlay networks and firewall rules
//!
//! // Dependency: implemented in clawz-worker, consumed by gateway::handlers

use std::collections::HashMap;
use std::pin::Pin;
use std::net::IpAddr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_core::Stream;
use http::HeaderMap;
use reqwest::Client;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::{
    agent::AgentState,
    channel::{ChannelCapabilities, ChannelConfig, IncomingMessage, OutgoingMessage},
    cost::CostRecord,
    deploy::{DeployConfig, DeployStatus, DeploymentInfo},
    governance::{ApprovalRequest, GovernanceResult},
    message::{ChatRequest, ChatResponse, Message, StreamChunk},
    mesh::PeerInfo,
    orchestration::{AgentHandle, AgentSpec, HealthStatus, SpawnConfig, ToolHandle, ToolType},
    tenant::{TenantContext, TenantId},
    tool::{ToolResult, ToolSchema},
};

// ── Provider ──────────────────────────────────────────────────────────────────

/// An LLM provider adapter (OpenAI, Anthropic, Gemini, Mistral, …).
/// // Implemented by: worker::providers (OpenAIProvider, AnthropicProvider, …)
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider name used for routing and metrics tags.
    fn name(&self) -> &str;

    /// List of model identifiers this provider can serve.
    fn models(&self) -> Vec<String>;

    /// Synchronous (blocking) chat completion.
    /// // Called by: worker::agent_runtime during pipeline execution.
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse>;

    /// Streaming chat completion returning a stream of `StreamChunk`.
    async fn chat_stream(
        &self,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    /// Whether this provider supports server-sent events / streaming.
    fn supports_streaming(&self) -> bool {
        true
    }

    /// Whether this provider exposes tool-calling in its API.
    fn supports_tools(&self) -> bool {
        true
    }

    /// Maximum context-window size for `model` (in tokens).
    fn context_window(&self, model: &str) -> u32;

    /// Estimate the cost for a given token usage.
    fn estimate_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> CostRecord;
}

// ── PipelineContext ───────────────────────────────────────────────────────────

/// Mutable context passed through every step of a processing pipeline.
///
/// Holds the accumulated state (messages, metadata, cost, governance)
/// as the request flows through pre-processing → governance → provider
/// → post-processing.
/// // Used by: worker::pipeline_engine
pub struct PipelineContext {
    pub agent_id: String,
    pub conversation_id: String,
    pub agent_state: AgentState,
    pub messages: Vec<Message>,
    pub metadata: HashMap<String, Value>,
    pub governance_result: Option<GovernanceResult>,
    pub cost_accumulated: f64,
    pub created_at: DateTime<Utc>,
    pub idempotency_store: Option<Box<dyn IdempotencyStore>>,
}

impl PipelineContext {
    pub fn new(agent_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            conversation_id: conversation_id.into(),
            agent_state: AgentState::default(),
            messages: Vec::new(),
            metadata: HashMap::new(),
            governance_result: None,
            cost_accumulated: 0.0,
            created_at: Utc::now(),
            idempotency_store: None,
        }
    }

    /// Insert arbitrary JSON metadata into the context.
    pub fn insert_meta(&mut self, key: impl Into<String>, value: Value) {
        self.metadata.insert(key.into(), value);
    }

    pub fn get_meta(&self, key: &str) -> Option<&Value> {
        self.metadata.get(key)
    }

    /// Add to the running cost total (in USD).
    pub fn add_cost(&mut self, cost_usd: f64) {
        self.cost_accumulated += cost_usd;
    }
}

// ── StepOutcome ───────────────────────────────────────────────────────────────

/// Decision produced by a `PipelineStep`.
/// // Consumed by: worker::pipeline_engine to decide flow control.
#[derive(Debug, Clone)]
pub enum StepOutcome {
    /// Continue to the next step in the pipeline.
    Continue,
    /// Halt the pipeline immediately (e.g. governance denied).
    Halt,
    /// Delegate to another agent identified by `target_agent`.
    Delegate { target_agent: String },
}

// ── PipelineStep ──────────────────────────────────────────────────────────────

/// A single stage in the agent request pipeline.
/// // Implemented by: worker::pipeline_steps (GovernanceStep, ProviderStep, …)
#[async_trait]
pub trait PipelineStep: Send + Sync {
    fn name(&self) -> &str;

    /// Execute the step, mutating `ctx` as needed.
    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome>;

    /// Undo side-effects if a later step fails.
    async fn rollback(&self, _ctx: &mut PipelineContext) -> Result<()> {
        Ok(())
    }

    /// Higher priority steps run first (default 0).
    fn priority(&self) -> i32 {
        0
    }
}

// ── ChannelMetadata / ChannelContext ──────────────────────────────────────────

/// Static metadata describing a channel plugin.
#[derive(Debug, Clone)]
pub struct ChannelMetadata {
    pub name: String,
    pub platform: String,
    pub version: String,
    pub author: String,
}

/// Runtime context passed to every `ChannelPlugin` method.
///
/// Holds the agent-scoped HTTP client so plugins can share connection
/// pooling instead of creating new clients per call.
#[derive(Clone)]
pub struct ChannelContext {
    pub config: ChannelConfig,
    pub agent_id: String,
    pub http_client: Client,
}

impl ChannelContext {
    pub fn new(config: ChannelConfig, agent_id: impl Into<String>) -> Self {
        Self {
            config,
            agent_id: agent_id.into(),
            http_client: Client::new(),
        }
    }
}

// ── ChannelPlugin ─────────────────────────────────────────────────────────────

/// Adapter for a messaging platform (Slack, Discord, Telegram, …).
/// // Implemented by: worker::channels (SlackPlugin, DiscordPlugin, …)
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    fn metadata(&self) -> ChannelMetadata;

    /// Poll or await incoming messages for this channel.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>>;

    /// Send an outgoing message to the platform.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()>;

    /// Handle a webhook push from the platform.
    async fn webhook(
        &self,
        payload: &[u8],
        headers: &HeaderMap,
    ) -> Result<Vec<IncomingMessage>>;

    /// Advertise supported features (threads, reactions, voice, …).
    fn capabilities(&self) -> ChannelCapabilities;
}

// ── ToolContext ───────────────────────────────────────────────────────────────

/// Runtime context passed to every `Tool::execute` call.
///
/// Includes the agent/conversation IDs and a shared HTTP client so
/// tools can make outbound requests without spinning up new clients.
#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: String,
    pub conversation_id: String,
    pub http_client: Client,
    /// Arbitrary tool-specific configuration.
    pub config: Value,
}

impl ToolContext {
    pub fn new(agent_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            conversation_id: conversation_id.into(),
            http_client: Client::new(),
            config: Value::Null,
        }
    }
}

// ── Tool ──────────────────────────────────────────────────────────────────────

/// An executable capability exposed to LLM agents.
/// // Implemented by: worker::tools (SearchTool, CodeTool, DeployTool, …)
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// JSON-Schema shape of the arguments this tool expects.
    fn schema(&self) -> ToolSchema;

    /// Execute the tool with the given arguments.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult>;

    /// Logical grouping for UI / registry organisation.
    fn category(&self) -> &str {
        "general"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }
}

// ── MemoryEntry / MemoryBackend ───────────────────────────────────────────────

/// A single retrieved memory item with vector-similarity metadata.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub key: String,
    pub value: Value,
    /// Cosine similarity score from the vector search (0.0 – 1.0).
    pub score: f64,
    pub timestamp: DateTime<Utc>,
}

impl MemoryEntry {
    pub fn new(key: impl Into<String>, value: Value, score: f64) -> Self {
        Self {
            key: key.into(),
            value,
            score,
            timestamp: Utc::now(),
        }
    }
}

/// Persistent storage backend for agent memory and conversation history.
///
/// Implementations may use Postgres + pgvector, Redis, or an external
/// vector DB. The trait is intentionally simple so swapping backends is
/// a one-file change.
/// // Implemented by: worker::memory (PgMemoryBackend, RedisMemoryBackend, …)
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Store a key-value pair, optionally with a vector embedding.
    async fn store(
        &self,
        agent_id: &str,
        key: &str,
        value: Value,
        embedding: Option<Vec<f32>>,
    ) -> Result<()>;

    /// Retrieve a single key by exact match.
    async fn retrieve(&self, agent_id: &str, key: &str) -> Result<Option<Value>>;

    /// Semantic search over stored embeddings.
    async fn search(
        &self,
        agent_id: &str,
        query_embedding: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>>;

    /// Fetch conversation history ordered by time.
    async fn get_conversation_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<Message>>;

    /// Append a single message to a conversation.
    async fn save_message(&self, conversation_id: &str, message: &Message) -> Result<()>;

    /// Delete a single key.
    async fn delete(&self, agent_id: &str, key: &str) -> Result<()>;

    /// Persist the full `AgentState` for the given agent.
    /// Default implementation stores it as JSON under the key `"__state__"`.
    async fn save_agent_state(
        &self,
        agent_id: &str,
        state: &crate::types::agent::AgentState,
    ) -> Result<()> {
        let value = serde_json::to_value(state)
            .map_err(|e| crate::error::ClawzError::Serialization(e.to_string()))?;
        self.store(agent_id, "__state__", value, None).await
    }
}

// ── Transport ─────────────────────────────────────────────────────────────────

/// A bidirectional connection to a mesh peer.
/// // Used by: worker::mesh, traits::TransportListener
pub struct TransportConnection {
    pub peer_id: String,
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
}

impl std::fmt::Debug for TransportConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportConnection")
            .field("peer_id", &self.peer_id)
            .finish_non_exhaustive()
    }
}

/// Accept side of a transport.
#[async_trait]
pub trait TransportListener: Send + Sync {
    async fn accept(&mut self) -> Result<TransportConnection>;
}

/// Mesh network transport abstraction (TCP, QUIC, WireGuard, …).
/// // Implemented by: worker::transports (TcpTransport, QuicTransport, …)
#[async_trait]
pub trait Transport: Send + Sync {
    fn name(&self) -> &str;

    /// Send `payload` to `peer` and return the response bytes.
    async fn send(&self, peer: &PeerInfo, payload: &[u8]) -> Result<Vec<u8>>;

    /// Bind to `addr` and return a listener.
    async fn listen(
        &self,
        addr: &str,
    ) -> Result<Box<dyn TransportListener>>;

    fn supports_streaming(&self) -> bool;
}

// ── GovernanceEngine ──────────────────────────────────────────────────────────

/// Policy evaluation and trust scoring engine.
/// // Implemented by: worker::governance (CelGovernanceEngine, …)
#[async_trait]
pub trait GovernanceEngine: Send + Sync {
    /// Evaluate whether `agent_id` may perform `action` in the given context.
    async fn evaluate(
        &self,
        agent_id: &str,
        action: &str,
        context: &Value,
    ) -> Result<GovernanceResult>;

    /// Current trust score for the agent (0.0 – 1.0).
    async fn get_trust_score(&self, agent_id: &str) -> Result<f64>;

    /// Adjust trust by `delta` and record the reason.
    async fn update_trust(&self, agent_id: &str, delta: f64, reason: &str) -> Result<()>;

    /// Check whether `action` is allowed under `policy_id`.
    async fn check_policy(&self, policy_id: &str, action: &str) -> Result<bool>;

    /// Submit an approval request and return its `id`.
    async fn request_approval(&self, request: ApprovalRequest) -> Result<String>;
}

// ── DeployAdapter ─────────────────────────────────────────────────────────────

/// Container / VM deployment adapter.
/// // Implemented by: worker::deploy (DockerAdapter, FlyAdapter, K8sAdapter, …)
#[async_trait]
pub trait DeployAdapter: Send + Sync {
    fn provider_name(&self) -> &str;

    async fn validate_config(&self, config: &DeployConfig) -> Result<()>;

    async fn deploy(&self, config: &DeployConfig, image: &str) -> Result<DeploymentInfo>;

    async fn status(&self, deployment_id: &str) -> Result<DeployStatus>;

    async fn stop(&self, deployment_id: &str) -> Result<()>;

    async fn logs(&self, deployment_id: &str, lines: usize) -> Result<Vec<String>>;

    async fn destroy(&self, deployment_id: &str) -> Result<()>;
}

// ── SaaSConnector ─────────────────────────────────────────────────────────────

/// Authentication mechanism required by a SaaS platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    OAuth2,
    ApiKey,
    Basic,
    Custom,
}

/// Generic connector for SaaS platforms (Salesforce, HubSpot, Stripe, …).
/// // Implemented by: worker::connectors (SalesforceConnector, StripeConnector, …)
#[async_trait]
pub trait SaaSConnector: Send + Sync {
    fn platform(&self) -> &str;
    fn auth_type(&self) -> AuthType;

    async fn authenticate(&self, credentials: &Value) -> Result<()>;

    async fn list_objects(
        &self,
        object_type: &str,
        filter: Option<&Value>,
    ) -> Result<Vec<Value>>;

    async fn get_object(&self, object_type: &str, id: &str) -> Result<Value>;

    async fn create_object(&self, object_type: &str, data: &Value) -> Result<Value>;

    async fn update_object(
        &self,
        object_type: &str,
        id: &str,
        data: &Value,
    ) -> Result<Value>;

    async fn delete_object(&self, object_type: &str, id: &str) -> Result<()>;

    async fn execute_action(&self, action: &str, params: &Value) -> Result<Value>;
}

// ── AgentScheduler ───────────────────────────────────────────────────────────

/// Lifecycle manager for agent containers.
/// // Implemented by: worker::scheduler (DockerScheduler, K8sScheduler, …)
#[async_trait]
pub trait AgentScheduler: Send + Sync {
    /// Spawn a new agent container from `spec`.
    async fn spawn_agent(&self, ctx: &TenantContext, spec: AgentSpec) -> Result<AgentHandle>;

    /// Stop and remove an agent container.
    async fn reap_agent(&self, handle: &AgentHandle) -> Result<()>;

    /// Find a warm (already running) agent that matches the requested capabilities.
    async fn find_warm(&self, ctx: &TenantContext, capabilities: &[String]) -> Option<AgentHandle>;

    /// List all agents for a tenant.
    async fn list_agents(&self, tenant_id: &str) -> Result<Vec<AgentHandle>>;

    /// Check health of a running agent.
    async fn health(&self, handle: &AgentHandle) -> Result<HealthStatus>;
}

// ── ToolOrchestrator ─────────────────────────────────────────────────────────

/// Lifecycle manager for tool containers.
/// // Implemented by: worker::tool_orchestrator (DockerToolOrchestrator, …)
#[async_trait]
pub trait ToolOrchestrator: Send + Sync {
    async fn spawn_tool(&self, tool_type: ToolType, config: SpawnConfig) -> Result<ToolHandle>;

    async fn reap_tool(&self, handle: &ToolHandle) -> Result<()>;

    async fn health(&self, handle: &ToolHandle) -> Result<HealthStatus>;

    async fn list_tools(&self) -> Result<Vec<ToolHandle>>;

    /// Gracefully stop all tools owned by `agent_id`, waiting up to `grace_secs`.
    async fn cascade_kill(&self, agent_id: &str, grace_secs: u64) -> Result<u32>;
}

// ── TenantMesh ───────────────────────────────────────────────────────────────

/// A tenant-isolated overlay network.
#[derive(Debug, Clone)]
pub struct MeshNetwork {
    pub name: String,
    pub subnet: String,
    pub tenant_id: TenantId,
}

/// Role of a container inside the mesh.
#[derive(Debug, Clone)]
pub enum MeshRole {
    Agent,
    Tool { owner_agent_id: String },
}

/// Firewall rule for inter-peer traffic inside a tenant network.
#[derive(Debug, Clone)]
pub enum FirewallRule {
    ToolToAgent { tool_ip: IpAddr, agent_ip: IpAddr },
    AgentToAgent { subnet: String },
    DenyInterTenant,
}

/// Per-tenant mesh network controller.
/// // Implemented by: worker::mesh (NetBirdMesh, TailscaleMesh, …)
#[async_trait]
pub trait TenantMesh: Send + Sync {
    /// Create a new isolated network for the tenant.
    async fn create_network(&self, tenant_id: &TenantId) -> Result<MeshNetwork>;

    /// Join a container to the network and return its assigned IP.
    async fn join(&self, network: &MeshNetwork, container_id: &str, role: MeshRole) -> Result<IpAddr>;

    /// Remove a container from the network.
    async fn leave(&self, network: &MeshNetwork, container_id: &str) -> Result<()>;

    /// Append a firewall rule.
    async fn add_firewall_rule(&self, network: &MeshNetwork, rule: FirewallRule) -> Result<()>;

    /// Tear down the tenant's network.
    async fn destroy_network(&self, tenant_id: &TenantId) -> Result<()>;
}

// ── IdempotencyStore (implemented in clawz-worker) ────────────────────────────

#[async_trait]
pub trait IdempotencyStore: Send + Sync {
    async fn check_and_record(&self, key: &IdempotencyKey, result: ToolResult) -> IdempotencyResult;
    async fn get(&self, key: &IdempotencyKey) -> Option<ToolResult>;
}

#[derive(Debug, Clone)]
pub struct IdempotencyKey {
    pub task_id: String,
    pub action_name: String,
    pub date: String,
    pub nonce: String,
}

impl IdempotencyKey {
    pub fn new(task_id: &str, action_name: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            action_name: action_name.to_string(),
            date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            nonce: uuid::Uuid::new_v4().to_string()[..8].to_string(),
        }
    }
    pub fn with_date(task_id: &str, action_name: &str, date: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            action_name: action_name.to_string(),
            date: date.to_string(),
            nonce: uuid::Uuid::new_v4().to_string()[..8].to_string(),
        }
    }
    pub fn with_nonce(task_id: &str, action_name: &str, nonce: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            action_name: action_name.to_string(),
            date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            nonce: nonce.to_string(),
        }
    }
    pub fn with_date_and_nonce(task_id: &str, action_name: &str, date: &str, nonce: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            action_name: action_name.to_string(),
            date: date.to_string(),
            nonce: nonce.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum IdempotencyResult {
    New,
    Cached(ToolResult),
    Expired,
}
