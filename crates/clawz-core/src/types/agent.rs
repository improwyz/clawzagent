//! Agent runtime types — status, configuration, state, and profile.
//!
//! These types are used by the worker scheduler to spawn agents and by
//! the gateway API to expose agent metadata to clients.
//!
//! // Dependency: used by worker::scheduler, gateway::agent_handlers, traits::PipelineContext

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::tool::ToolSchema;

// ── AgentStatus ───────────────────────────────────────────────────────────────

/// Lifecycle states for an agent container or process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    #[default]
    Idle,
    Running,
    Stopped,
    Error,
    Paused,
    /// Delegating a sub-task to another agent in the mesh.
    Delegating,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Idle => write!(f, "idle"),
            AgentStatus::Running => write!(f, "running"),
            AgentStatus::Stopped => write!(f, "stopped"),
            AgentStatus::Error => write!(f, "error"),
            AgentStatus::Paused => write!(f, "paused"),
            AgentStatus::Delegating => write!(f, "delegating"),
        }
    }
}

// ── AgentConfig ───────────────────────────────────────────────────────────────

/// Static configuration for an agent — model, prompt, tools, sampling params.
/// // Dependency: stored as JSONB in db::DbAgent.config, used by worker::agent_runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: Uuid,
    pub name: String,
    /// Model identifier in provider notation (e.g. "claude-sonnet-4-5").
    pub model: String,
    /// System prompt prepended to every conversation.
    pub system_prompt: String,
    /// Names of tools available to this agent.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Sampling temperature (0.0 = deterministic, 2.0 = very random).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Max tokens per completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Free-form metadata for UI labels, tags, etc.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AgentConfig {
    pub fn new(name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            model: model.into(),
            system_prompt: String::new(),
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }
}

// ── AgentState ────────────────────────────────────────────────────────────────

/// Mutable runtime state for an agent — updated on every turn.
/// // Dependency: persisted by traits::MemoryBackend, displayed by gateway dashboards.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    pub status: AgentStatus,
    /// Current task description (for UI / observability).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AgentState {
    pub fn idle() -> Self {
        Self {
            status: AgentStatus::Idle,
            ..Default::default()
        }
    }

    pub fn running(task: impl Into<String>) -> Self {
        Self {
            status: AgentStatus::Running,
            current_task: Some(task.into()),
            last_message_at: Some(Utc::now()),
            ..Default::default()
        }
    }

    pub fn set_status(&mut self, status: AgentStatus) {
        self.status = status;
    }

    pub fn mark_message(&mut self) {
        self.last_message_at = Some(Utc::now());
    }
}

// ── AgentProfile — config + runtime stats ─────────────────────────────────────

/// Aggregated view of an agent — config, state, tool schemas, and usage totals.
/// // Dependency: returned by gateway::agent_handlers for the admin UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub config: AgentConfig,
    pub state: AgentState,
    pub resolved_tools: Vec<ToolSchema>,
    pub total_requests: u64,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub total_cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentProfile {
    pub fn new(config: AgentConfig) -> Self {
        let now = Utc::now();
        Self {
            config,
            state: AgentState::idle(),
            resolved_tools: Vec::new(),
            total_requests: 0,
            total_tokens_in: 0,
            total_tokens_out: 0,
            total_cost_usd: 0.0,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn record_request(&mut self, tokens_in: u64, tokens_out: u64, cost: f64) {
        self.total_requests += 1;
        self.total_tokens_in += tokens_in;
        self.total_tokens_out += tokens_out;
        self.total_cost_usd += cost;
        self.updated_at = Utc::now();
    }
}
