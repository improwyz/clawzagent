//! Container and tool lifecycle management.
//!
//! The [`LifecycleManager`] tracks [`ToolHandle`]s that are associated with
//! running agents and provides several garbage-collection routines:
//!
//! - **Cascade kill**: remove every tool owned by a given agent (used when the
//!   agent itself is reaped).
//! - **Idle reap**: remove tools that have not been used for longer than the
//!   configured idle timeout.
//! - **Orphan sweep**: remove tools whose owning agent is no longer known to
//!   the scheduler.
//!
//! # Role in the worker crate
//!
//! This module sits *beside* the scheduler implementations; while schedulers
//! manage **agents**, the `LifecycleManager` manages the **tools** those agents
//! create. It is typically instantiated once per worker and shared across
//! tasks via [`Arc`].
//!
//! # Architecture context
//!
//! Workers execute via the runtime module. The 3 crates form a 3-tier
//! architecture: gateway (API layer) → worker (execution layer) → core
//! (shared types/traits).
//!
//! # Key dependencies
//!
//! - `clawz_core::types::orchestration::ToolHandle` — the domain type for
//!   individual tool instances.
//! - `tokio::sync::RwLock` — async-aware lock because every public method is
//!   async and may be called from multiple Tokio tasks concurrently.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

// Dependency: core crate — shared tool handle definition
use clawz_core::types::orchestration::ToolHandle;

/// Manages the lifespan of [`ToolHandle`]s created by agents.
///
/// Tools are registered when spawned and later removed by one of several
/// cleanup strategies (cascade kill, idle reap, orphan sweep).
///
/// # Thread safety
///
/// All state is protected by a [`tokio::sync::RwLock`] so the manager can be
/// held in an [`Arc`] and accessed from multiple async tasks safely.
pub struct LifecycleManager {
    /// Registry of all known tools. Keyed by the tool's string identifier
    /// (typically `ToolHandle.id.to_string()`).
    tools: Arc<RwLock<HashMap<String, ToolHandle>>>,
    /// Duration after which an untouched tool is considered idle and may be
    /// removed by [`Self::reap_idle_tools`].
    idle_timeout: Duration,
}

impl LifecycleManager {
    /// Create a new manager with the given idle timeout.
    ///
    /// # Choosing a timeout
    ///
    /// A timeout of `0` means [`reap_idle_tools`](Self::reap_idle_tools) will
    /// remove *every* tool on the next call, which is useful in tests but
    /// destructive in production. Typical production values range from 5 to
    /// 30 minutes.
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            idle_timeout,
        }
    }

    /// Register a newly created tool so the manager can track it.
    ///
    /// If a tool with the same ID already exists, it will be overwritten.
    /// Callers should ensure IDs are unique (e.g. UUID-based).
    pub async fn register_tool(&self, tool: ToolHandle) {
        self.tools.write().await.insert(tool.id.to_string(), tool);
    }

    /// Remove every tool owned by `agent_id`.
    ///
    /// This is intended to be called immediately after an agent is reaped so
    /// that dangling tool resources are not left behind.
    ///
    /// # Returns
    ///
    /// The number of tools that were removed.
    pub async fn cascade_kill_tools(&self, agent_id: &str) -> u32 {
        let mut tools = self.tools.write().await;
        // Collect keys first so we do not hold an iterator across the remove.
        let to_remove: Vec<String> = tools
            .iter()
            .filter(|(_, t)| t.owner_agent_id == agent_id)
            .map(|(k, _)| k.clone())
            .collect();
        let count = to_remove.len() as u32;
        for key in to_remove {
            tools.remove(&key);
        }
        count
    }

    /// List all tools whose `owner_agent_id` matches the supplied value.
    ///
    /// Useful for debugging and for the orphan sweep's inverse check.
    pub async fn list_tools_for_agent(&self, agent_id: &str) -> Vec<ToolHandle> {
        self.tools
            .read()
            .await
            .values()
            .filter(|t| t.owner_agent_id == agent_id)
            .cloned()
            .collect()
    }

    /// Remove tools that have been idle longer than [`Self::idle_timeout`].
    ///
    /// # Returns
    ///
    /// The number of tools reaped.
    pub async fn reap_idle_tools(&self) -> u32 {
        let mut tools = self.tools.write().await;
        let idle_secs = self.idle_timeout.as_secs();
        // Snapshot keys to avoid iterator invalidation during removal.
        let to_remove: Vec<String> = tools
            .iter()
            .filter(|(_, t)| t.idle_seconds() > idle_secs)
            .map(|(k, _)| k.clone())
            .collect();
        let count = to_remove.len() as u32;
        for key in to_remove {
            tools.remove(&key);
        }
        count
    }

    /// Remove tools whose owning agent is **not** present in `known_agents`.
    ///
    /// This should be run periodically (e.g. by a background Tokio task)
    /// to clean up tools left behind when an agent crashes or is removed by an
    /// external operator.
    ///
    /// # Returns
    ///
    /// The number of orphaned tools removed.
    pub async fn orphan_sweep(&self, known_agents: &[String]) -> u32 {
        let mut tools = self.tools.write().await;
        // Snapshot keys to avoid iterator invalidation.
        let orphans: Vec<String> = tools
            .iter()
            .filter(|(_, t)| !known_agents.contains(&t.owner_agent_id))
            .map(|(k, _)| k.clone())
            .collect();
        let count = orphans.len() as u32;
        for key in orphans {
            tools.remove(&key);
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use clawz_core::types::orchestration::ToolType;

    #[tokio::test]
    async fn cascade_kill_removes_all_tools() {
        let manager = LifecycleManager::new(Duration::from_secs(5));
        let tool1 = ToolHandle::new("agent-1".into(), ToolType::Browser, "10.0.0.50".into());
        let tool2 = ToolHandle::new("agent-1".into(), ToolType::Sandbox, "10.0.0.51".into());
        let tool3 = ToolHandle::new("agent-2".into(), ToolType::Browser, "10.0.0.52".into());
        manager.register_tool(tool1).await;
        manager.register_tool(tool2).await;
        manager.register_tool(tool3).await;

        let killed = manager.cascade_kill_tools("agent-1").await;
        assert_eq!(killed, 2);

        let remaining = manager.list_tools_for_agent("agent-2").await;
        assert_eq!(remaining.len(), 1);
    }

    #[tokio::test]
    async fn idle_reap_removes_stale_tools() {
        // Timeout of 0 means every tool is immediately considered idle.
        let manager = LifecycleManager::new(Duration::from_secs(0));
        let mut tool = ToolHandle::new("agent-1".into(), ToolType::McpBridge, "10.0.0.50".into());
        // Artificially set last activity to 10 minutes ago.
        tool.last_activity = Utc::now() - chrono::Duration::seconds(600);
        manager.register_tool(tool).await;

        let reaped = manager.reap_idle_tools().await;
        assert_eq!(reaped, 1);
    }

    #[tokio::test]
    async fn orphan_sweep_removes_unknown_agents() {
        let manager = LifecycleManager::new(Duration::from_secs(60));
        let tool1 = ToolHandle::new("agent-alive".into(), ToolType::Browser, "10.0.0.50".into());
        let tool2 = ToolHandle::new("agent-dead".into(), ToolType::Sandbox, "10.0.0.51".into());
        manager.register_tool(tool1).await;
        manager.register_tool(tool2).await;

        let known = vec!["agent-alive".to_string()];
        let swept = manager.orphan_sweep(&known).await;
        assert_eq!(swept, 1);
        assert_eq!(manager.list_tools_for_agent("agent-alive").await.len(), 1);
        assert_eq!(manager.list_tools_for_agent("agent-dead").await.len(), 0);
    }
}
