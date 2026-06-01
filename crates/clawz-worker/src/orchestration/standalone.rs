//! Standalone scheduler implementation for single-node agent orchestration.
//!
//! This module provides an in-process [`AgentScheduler`] that does **not**
//! require Docker. Agents are modelled as lightweight [`AgentHandle`] entries
//! kept in a thread-safe in-memory map. It is the default backend when the
//! system is configured for [`DeploymentMode::Standalone`].
//!
//! # When to use
//!
//! - Local development or testing where Docker is unavailable.
//! - Single-node deployments that prioritise low overhead over isolation.
//! - CI pipelines that need a deterministic, container-free execution path.
//!
//! # Architecture context
//!
//! Workers execute via the runtime module. The 3 crates form a 3-tier
//! architecture: gateway (API layer) → worker (execution layer) → core
//! (shared types/traits).
//!
//! # Key differences from [`BollardScheduler`]
//!
//! | Aspect           | Standalone                     | Bollard                        |
//! |------------------|--------------------------------|--------------------------------|
//! | Isolation        | Process-level (shared)         | Container-level (cgroup/namespace) |
//! | Resource limits  | Enforced by the OS / runtime   | Enforced by Docker HostConfig  |
//! | `container_id`   | Always `None`                  | Populated with Docker ID       |
//! | Health memory    | Synthetic 10 %                 | 0 % (not exposed by inspect)   |

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

// Dependency: core crate — shared error types, scheduler trait, and orchestration / tenant types
use clawz_core::error::{ClawzError, Result};
use clawz_core::traits::AgentScheduler;
use clawz_core::types::orchestration::{AgentHandle, AgentSpec, ContainerState, HealthStatus};
use clawz_core::types::tenant::TenantContext;

/// In-memory standalone agent scheduler with capacity limits.
///
/// Uses `Arc<RwLock<HashMap>>` for thread-safe agent storage.
/// Suitable for single-node deployments where agents run as
/// lightweight tasks within the same process.
///
/// # Concurrency
///
/// Read-heavy operations (`find_warm`, `list_agents`, `health`) take a read
/// lock; mutations (`spawn_agent`, `reap_agent`) take a write lock. This
/// matches the access patterns of the [`AgentScheduler`] trait.
pub struct StandaloneScheduler {
    /// In-memory registry of active agents. Keyed by agent [`Uuid`].
    agents: Arc<RwLock<HashMap<Uuid, AgentHandle>>>,
    /// Maximum number of agents this scheduler may host concurrently.
    max_agents: usize,
}

impl StandaloneScheduler {
    /// Create a new standalone scheduler with the given capacity limit.
    ///
    /// # Choosing a limit
    ///
    /// In standalone mode the limit is primarily a safety guard against
    /// unbounded growth; the real concurrency bound is the Tokio runtime's
    /// thread pool.
    pub fn new(max_agents: usize) -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            max_agents,
        }
    }

    /// Return the current number of registered agents.
    pub fn agent_count(&self) -> usize {
        let agents = self.agents.read().unwrap();
        agents.len()
    }

    /// Return the configured maximum agent capacity.
    pub fn max_agents(&self) -> usize {
        self.max_agents
    }
}

#[async_trait]
impl AgentScheduler for StandaloneScheduler {
    /// Spawn a new in-process agent.
    ///
    /// Because there is no external container to create, this simply:
    /// 1. Checks capacity.
    /// 2. Generates an ID and mesh IP placeholder.
    /// 3. Builds an [`AgentHandle`] and inserts it into the map.
    ///
    /// The agent immediately transitions to [`ContainerState::Ready`]
    /// (there is no "starting" phase because no container boot is required).
    async fn spawn_agent(&self, ctx: &TenantContext, spec: AgentSpec) -> Result<AgentHandle> {
        // Capacity guard: use a scoped read lock so we release it before
        // acquiring the write lock later, avoiding deadlocks.
        {
            let agents = self.agents.read().unwrap();
            if agents.len() >= self.max_agents {
                return Err(ClawzError::Orchestration(format!(
                    "max agent capacity reached ({})",
                    self.max_agents
                )));
            }
        }

        // Generate a unique agent identifier and mesh IP (simplified for standalone)
        let agent_id = format!("agent-{}", Uuid::new_v4());
        let mesh_ip = "127.0.0.1".to_string();

        let mut handle = AgentHandle::new(ctx.tenant_id.clone(), agent_id, mesh_ip);
        handle.capabilities = spec.capabilities.clone();
        // Standalone agents are immediately ready because there is no container boot.
        handle.state = ContainerState::Ready;

        let mut agents = self.agents.write().unwrap();
        agents.insert(handle.id, handle.clone());

        Ok(handle)
    }

    /// Remove the agent from the internal registry.
    ///
    /// Unlike [`BollardScheduler::reap_agent`], there is no external container
    /// to stop, so this is a pure map removal.
    async fn reap_agent(&self, handle: &AgentHandle) -> Result<()> {
        let mut agents = self.agents.write().unwrap();
        match agents.get_mut(&handle.id) {
            Some(h) => {
                // Mark stopping for consistency with container-backed schedulers.
                h.state = ContainerState::Stopping;
                agents.remove(&handle.id);
                Ok(())
            }
            None => Err(ClawzError::NotFound {
                entity: "agent".into(),
                id: handle.id.to_string(),
            }),
        }
    }

    /// Find a warm agent that matches all requested capabilities for the tenant.
    ///
    /// # Matching rules
    /// - Tenant must match (isolation).
    /// - Agent must be alive.
    /// - Agent must contain every requested capability (subset semantics).
    async fn find_warm(&self, ctx: &TenantContext, capabilities: &[String]) -> Option<AgentHandle> {
        let agents = self.agents.read().unwrap();
        for (_, handle) in agents.iter() {
            if handle.tenant_id != ctx.tenant_id {
                continue;
            }
            if !handle.is_alive() {
                continue;
            }
            // Check if agent has all requested capabilities
            let has_all = capabilities
                .iter()
                .all(|cap| handle.capabilities.contains(cap));
            if has_all {
                return Some(handle.clone());
            }
        }
        None
    }

    /// List all agents whose `tenant_id` equals the supplied value.
    async fn list_agents(&self, tenant_id: &str) -> Result<Vec<AgentHandle>> {
        let agents = self.agents.read().unwrap();
        let list: Vec<AgentHandle> = agents
            .values()
            .filter(|h| h.tenant_id.as_str() == tenant_id)
            .cloned()
            .collect();
        Ok(list)
    }

    /// Report synthetic health for the agent.
    ///
    /// Because there is no external runtime (Docker) to query, readiness is
    /// derived from the agent's [`ContainerState`] and liveness from
    /// [`AgentHandle::is_alive`]. Memory usage is reported as a fixed low
    /// value because we do not instrument the process heap here.
    async fn health(&self, handle: &AgentHandle) -> Result<HealthStatus> {
        let agents = self.agents.read().unwrap();
        let agent = agents.get(&handle.id).ok_or_else(|| ClawzError::NotFound {
            entity: "agent".into(),
            id: handle.id.to_string(),
        })?;

        let readiness = match agent.state {
            ContainerState::Ready | ContainerState::Running => 1.0,
            ContainerState::Draining => 0.5,
            _ => 0.0,
        };

        let liveness = agent.is_alive();

        Ok(HealthStatus {
            readiness,
            liveness,
            // Standalone: always reports available because we do not track
            // in-process task slots separately from Tokio's executor.
            tool_slots_available: 5,
            // Standalone: synthetic low value because accurate per-agent
            // memory accounting would require OS-specific APIs.
            memory_usage_percent: 10.0,
            last_check: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::orchestration::AgentSpec;
    use clawz_core::types::tenant::{Role, TenantContext, TenantId};

    fn test_context() -> TenantContext {
        TenantContext::new(TenantId::new("test-tenant"), Role::Operator)
    }

    fn test_spec() -> AgentSpec {
        AgentSpec {
            capabilities: vec!["chat".to_string(), "search".to_string()],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_spawn_agent() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx = test_context();
        let spec = test_spec();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();
        assert_eq!(handle.tenant_id.as_str(), "test-tenant");
        assert!(handle.is_alive());
        assert_eq!(scheduler.agent_count(), 1);
    }

    #[tokio::test]
    async fn test_max_agents_capacity() {
        let scheduler = StandaloneScheduler::new(2);
        let ctx = test_context();

        scheduler
            .spawn_agent(&ctx, AgentSpec::default())
            .await
            .unwrap();
        scheduler
            .spawn_agent(&ctx, AgentSpec::default())
            .await
            .unwrap();

        let result = scheduler.spawn_agent(&ctx, AgentSpec::default()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ClawzError::Orchestration(_)));
        assert_eq!(scheduler.agent_count(), 2);
    }

    #[tokio::test]
    async fn test_reap_agent() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx = test_context();
        let spec = test_spec();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();
        assert_eq!(scheduler.agent_count(), 1);

        scheduler.reap_agent(&handle).await.unwrap();
        assert_eq!(scheduler.agent_count(), 0);

        // Reaping again should fail
        let result = scheduler.reap_agent(&handle).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_find_warm() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx = test_context();
        let spec = AgentSpec {
            capabilities: vec!["chat".to_string(), "image_gen".to_string()],
            ..Default::default()
        };

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();

        // Exact capability match
        let found = scheduler.find_warm(&ctx, &["chat".to_string()]).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, handle.id);

        // Multiple capabilities — agent has all
        let found = scheduler
            .find_warm(&ctx, &["chat".to_string(), "image_gen".to_string()])
            .await;
        assert!(found.is_some());

        // Missing capability
        let found = scheduler.find_warm(&ctx, &["missing".to_string()]).await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_find_warm_different_tenant() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx1 = TenantContext::new(TenantId::new("tenant-1"), Role::Operator);
        let ctx2 = TenantContext::new(TenantId::new("tenant-2"), Role::Operator);

        scheduler
            .spawn_agent(&ctx1, AgentSpec::default())
            .await
            .unwrap();

        let found = scheduler.find_warm(&ctx2, &[]).await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_list_agents() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx1 = TenantContext::new(TenantId::new("tenant-1"), Role::Operator);
        let ctx2 = TenantContext::new(TenantId::new("tenant-2"), Role::Operator);

        scheduler
            .spawn_agent(&ctx1, AgentSpec::default())
            .await
            .unwrap();
        scheduler
            .spawn_agent(&ctx1, AgentSpec::default())
            .await
            .unwrap();
        scheduler
            .spawn_agent(&ctx2, AgentSpec::default())
            .await
            .unwrap();

        let list = scheduler.list_agents("tenant-1").await.unwrap();
        assert_eq!(list.len(), 2);

        let list = scheduler.list_agents("tenant-2").await.unwrap();
        assert_eq!(list.len(), 1);

        let list = scheduler.list_agents("tenant-3").await.unwrap();
        assert_eq!(list.len(), 0);
    }

    #[tokio::test]
    async fn test_health() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx = test_context();
        let spec = AgentSpec::default();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();
        let health = scheduler.health(&handle).await.unwrap();

        assert!(health.liveness);
        assert_eq!(health.readiness, 1.0);
        assert!(health.is_ready());
    }

    #[tokio::test]
    async fn test_health_not_found() {
        let scheduler = StandaloneScheduler::new(5);
        let handle = AgentHandle::new(
            TenantId::new("other"),
            "ghost".to_string(),
            "10.0.0.1".to_string(),
        );

        let result = scheduler.health(&handle).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ClawzError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_find_warm_not_alive() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx = test_context();
        let spec = AgentSpec::default();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();
        scheduler.reap_agent(&handle).await.unwrap();

        let found = scheduler.find_warm(&ctx, &[]).await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_concurrent_spawn() {
        let scheduler = Arc::new(StandaloneScheduler::new(10));
        let ctx = test_context();

        let mut handles = vec![];
        for _ in 0..10 {
            let s = scheduler.clone();
            let c = ctx.clone();
            handles.push(tokio::spawn(async move {
                s.spawn_agent(&c, AgentSpec::default()).await
            }));
        }

        let results = futures_util::future::join_all(handles).await;
        let successes: Vec<_> = results
            .iter()
            .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
            .collect();
        assert_eq!(successes.len(), 10);
        assert_eq!(scheduler.agent_count(), 10);
    }

    #[tokio::test]
    async fn test_scheduler_constructor() {
        let scheduler = StandaloneScheduler::new(100);
        assert_eq!(scheduler.max_agents(), 100);
        assert_eq!(scheduler.agent_count(), 0);
    }

    #[tokio::test]
    async fn test_find_warm_empty_capabilities() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx = test_context();
        let spec = AgentSpec::default();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();

        // Empty capability list should match any alive agent
        let found = scheduler.find_warm(&ctx, &[]).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, handle.id);
    }

    #[tokio::test]
    async fn test_health_draining_state() {
        let scheduler = StandaloneScheduler::new(5);
        let ctx = test_context();
        let spec = AgentSpec::default();

        let mut handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();

        // Manually transition to Draining
        {
            let mut agents = scheduler.agents.write().unwrap();
            if let Some(h) = agents.get_mut(&handle.id) {
                h.state = ContainerState::Draining;
            }
        }

        // Re-read handle from storage to get updated state
        {
            let agents = scheduler.agents.read().unwrap();
            handle = agents.get(&handle.id).unwrap().clone();
        }

        let health = scheduler.health(&handle).await.unwrap();
        assert!(health.liveness); // Draining is alive
        assert_eq!(health.readiness, 0.5);
        assert!(!health.is_ready()); // readiness < 0.6
    }
}
