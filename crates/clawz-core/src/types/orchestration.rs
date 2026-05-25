//! Orchestration types — container handles, spawn configs, and health status.
//!
//! These types bridge the scheduler (which decides *what* to run) with
//! the deploy adapter (which decides *how* to run it).
//!
//! // Dependency: used by worker::scheduler, worker::tool_orchestrator, gateway::orchestration_handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

use crate::types::tenant::TenantId;

/// Container lifecycle state
/// // Dependency: tracked by worker::scheduler and worker::tool_orchestrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    Starting,
    Ready,
    Running,
    Draining,
    Stopping,
    Stopped,
    Failed,
}

impl fmt::Display for ContainerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContainerState::Starting => write!(f, "starting"),
            ContainerState::Ready => write!(f, "ready"),
            ContainerState::Running => write!(f, "running"),
            ContainerState::Draining => write!(f, "draining"),
            ContainerState::Stopping => write!(f, "stopping"),
            ContainerState::Stopped => write!(f, "stopped"),
            ContainerState::Failed => write!(f, "failed"),
        }
    }
}

/// Tool type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolType {
    Browser,
    Sandbox,
    McpBridge,
    Connectors,
    Infra,
}

impl fmt::Display for ToolType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolType::Browser => write!(f, "browser"),
            ToolType::Sandbox => write!(f, "sandbox"),
            ToolType::McpBridge => write!(f, "mcp_bridge"),
            ToolType::Connectors => write!(f, "connectors"),
            ToolType::Infra => write!(f, "infra"),
        }
    }
}

/// Handle to a running agent container
/// // Dependency: returned by traits::AgentScheduler::spawn_agent, tracked in worker::scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandle {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub agent_id: String,
    pub container_id: Option<String>,
    pub mesh_ip: String,
    pub state: ContainerState,
    pub capabilities: Vec<String>,
    pub spawned_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

impl AgentHandle {
    /// Create a new agent handle in Starting state
    pub fn new(tenant_id: TenantId, agent_id: String, mesh_ip: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            tenant_id,
            agent_id,
            container_id: None,
            mesh_ip,
            state: ContainerState::Starting,
            capabilities: Vec::new(),
            spawned_at: now,
            last_heartbeat: now,
        }
    }

    /// Transition agent to a new state
    pub fn transition(&mut self, new_state: ContainerState) {
        self.state = new_state;
    }

    /// Check if agent is alive (Ready, Running, or Draining)
    pub fn is_alive(&self) -> bool {
        matches!(
            self.state,
            ContainerState::Ready | ContainerState::Running | ContainerState::Draining
        )
    }

    /// Check if heartbeat is stale (hasn't been updated within max_age_secs)
    pub fn heartbeat_stale(&self, max_age_secs: u64) -> bool {
        let age = Utc::now()
            .signed_duration_since(self.last_heartbeat)
            .num_seconds();
        age > max_age_secs as i64
    }
}

/// Handle to a running tool container
/// // Dependency: returned by traits::ToolOrchestrator::spawn_tool, tracked in worker::tool_orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolHandle {
    pub id: Uuid,
    pub owner_agent_id: String,
    pub tool_type: ToolType,
    pub container_id: Option<String>,
    pub mesh_ip: String,
    pub state: ContainerState,
    pub spawned_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

impl ToolHandle {
    /// Create a new tool handle in Starting state
    pub fn new(owner_agent_id: String, tool_type: ToolType, mesh_ip: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            owner_agent_id,
            tool_type,
            container_id: None,
            mesh_ip,
            state: ContainerState::Starting,
            spawned_at: now,
            last_activity: now,
        }
    }

    /// Get seconds elapsed since last activity
    pub fn idle_seconds(&self) -> u64 {
        Utc::now()
            .signed_duration_since(self.last_activity)
            .num_seconds()
            .max(0) as u64
    }
}

/// Specification for spawning an agent
/// // Dependency: passed to traits::AgentScheduler::spawn_agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    pub memory_mb: u64,
    pub cpu_millicores: u64,
    pub image: String,
    pub capabilities: Vec<String>,
    pub max_tools: u32,
    pub idle_timeout_secs: u64,
}

impl Default for AgentSpec {
    fn default() -> Self {
        Self {
            memory_mb: 256,
            cpu_millicores: 500,
            image: "clawz/agent:latest".to_string(),
            capabilities: Vec::new(),
            max_tools: 5,
            idle_timeout_secs: 300,
        }
    }
}

/// Configuration for spawning a container
/// // Dependency: passed to traits::ToolOrchestrator::spawn_tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnConfig {
    pub memory_mb: u64,
    pub cpu_millicores: u64,
    pub image: String,
    pub env: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub network: String,
}

/// Health status of a running container
/// // Dependency: returned by traits::AgentScheduler::health and traits::ToolOrchestrator::health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub readiness: f32,
    pub liveness: bool,
    pub tool_slots_available: u32,
    pub memory_usage_percent: f32,
    pub last_check: DateTime<Utc>,
}

impl HealthStatus {
    /// Check if container is ready for work (liveness && readiness >= 0.6)
    pub fn is_ready(&self) -> bool {
        self.liveness && self.readiness >= 0.6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_handle_creation() {
        let tenant_id = TenantId::default();
        let agent_id = "test-agent".to_string();
        let mesh_ip = "10.0.0.1".to_string();

        let handle = AgentHandle::new(tenant_id.clone(), agent_id.clone(), mesh_ip.clone());

        assert_eq!(handle.agent_id, agent_id);
        assert_eq!(handle.mesh_ip, mesh_ip);
        assert_eq!(handle.state, ContainerState::Starting);
        assert_eq!(handle.container_id, None);
        assert!(handle.capabilities.is_empty());
    }

    #[test]
    fn tool_handle_creation() {
        let owner_agent_id = "agent-1".to_string();
        let tool_type = ToolType::Browser;
        let mesh_ip = "10.0.0.2".to_string();

        let handle = ToolHandle::new(owner_agent_id.clone(), tool_type, mesh_ip.clone());

        assert_eq!(handle.owner_agent_id, owner_agent_id);
        assert_eq!(handle.tool_type, tool_type);
        assert_eq!(handle.mesh_ip, mesh_ip);
        assert_eq!(handle.state, ContainerState::Starting);
        assert_eq!(handle.container_id, None);
    }

    #[test]
    fn agent_spec_defaults() {
        let spec = AgentSpec::default();

        assert_eq!(spec.memory_mb, 256);
        assert_eq!(spec.cpu_millicores, 500);
        assert_eq!(spec.image, "clawz/agent:latest");
        assert!(spec.capabilities.is_empty());
        assert_eq!(spec.max_tools, 5);
        assert_eq!(spec.idle_timeout_secs, 300);
    }

    #[test]
    fn container_state_transitions() {
        let tenant_id = TenantId::default();
        let mut handle = AgentHandle::new(tenant_id, "agent-1".to_string(), "10.0.0.1".to_string());

        assert_eq!(handle.state, ContainerState::Starting);

        handle.transition(ContainerState::Ready);
        assert_eq!(handle.state, ContainerState::Ready);

        handle.transition(ContainerState::Running);
        assert_eq!(handle.state, ContainerState::Running);

        handle.transition(ContainerState::Stopping);
        assert_eq!(handle.state, ContainerState::Stopping);

        handle.transition(ContainerState::Stopped);
        assert_eq!(handle.state, ContainerState::Stopped);
    }

    #[test]
    fn agent_is_alive() {
        let tenant_id = TenantId::default();
        let mut handle = AgentHandle::new(tenant_id, "agent-1".to_string(), "10.0.0.1".to_string());

        assert!(!handle.is_alive()); // Starting state

        handle.transition(ContainerState::Ready);
        assert!(handle.is_alive());

        handle.transition(ContainerState::Running);
        assert!(handle.is_alive());

        handle.transition(ContainerState::Draining);
        assert!(handle.is_alive());

        handle.transition(ContainerState::Stopped);
        assert!(!handle.is_alive());

        handle.transition(ContainerState::Failed);
        assert!(!handle.is_alive());
    }

    #[test]
    fn tool_idle_seconds() {
        let handle = ToolHandle::new("agent-1".to_string(), ToolType::Sandbox, "10.0.0.2".to_string());

        let idle = handle.idle_seconds();
        assert!(idle < 2); // Should be nearly 0
    }

    #[test]
    fn health_status_ready() {
        let status = HealthStatus {
            readiness: 0.8,
            liveness: true,
            tool_slots_available: 3,
            memory_usage_percent: 45.0,
            last_check: Utc::now(),
        };

        assert!(status.is_ready());

        let unready = HealthStatus {
            readiness: 0.5,
            liveness: true,
            tool_slots_available: 3,
            memory_usage_percent: 45.0,
            last_check: Utc::now(),
        };

        assert!(!unready.is_ready());

        let dead = HealthStatus {
            readiness: 0.8,
            liveness: false,
            tool_slots_available: 3,
            memory_usage_percent: 45.0,
            last_check: Utc::now(),
        };

        assert!(!dead.is_ready());
    }

    #[test]
    fn container_state_display() {
        assert_eq!(ContainerState::Starting.to_string(), "starting");
        assert_eq!(ContainerState::Ready.to_string(), "ready");
        assert_eq!(ContainerState::Running.to_string(), "running");
        assert_eq!(ContainerState::Failed.to_string(), "failed");
    }

    #[test]
    fn tool_type_display() {
        assert_eq!(ToolType::Browser.to_string(), "browser");
        assert_eq!(ToolType::Sandbox.to_string(), "sandbox");
        assert_eq!(ToolType::McpBridge.to_string(), "mcp_bridge");
        assert_eq!(ToolType::Connectors.to_string(), "connectors");
        assert_eq!(ToolType::Infra.to_string(), "infra");
    }
}
