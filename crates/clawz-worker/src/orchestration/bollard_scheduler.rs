//! Docker-based scheduler implementation using the Bollard crate.
//!
//! Spawns agents as real Docker containers, tracks them via container IDs,
//! and implements the [`AgentScheduler`] trait from `clawz_core`.
//!
//! # Role in the worker crate
//!
//! This is the heavy-duty backend used when the system is configured for
//! [`DeploymentMode::Micro`] or [`DeploymentMode::Elastic`]. Each agent gets
//! its own isolated container with configurable CPU, memory, and PID limits.
//!
//! # Key dependencies
//!
//! - `bollard` — async Docker API client.
//! - `clawz_core::traits::AgentScheduler` — trait this module implements.
//! - `clawz_core::types::orchestration` — shared handle / spec / state types.
//! - `futures_util::StreamExt` — consumed when pulling images via Bollard streams.
//!
//! # Architecture note
//!
//! Workers execute via the runtime module. The 3 crates form a 3-tier
//! architecture: gateway (API layer) → worker (execution layer) → core
//! (shared types/traits).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{
    Config as ContainerConfig, CreateContainerOptions, InspectContainerOptions,
    RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::HostConfig;
use chrono::Utc;
use futures_util::StreamExt;
use uuid::Uuid;

// Dependency: core crate — shared error types, scheduler trait, and orchestration types
use clawz_core::error::{ClawzError, Result};
use clawz_core::traits::AgentScheduler;
use clawz_core::types::orchestration::{AgentHandle, AgentSpec, ContainerState, HealthStatus};
use clawz_core::types::tenant::TenantContext;

/// Docker-based agent scheduler using the Bollard Docker API.
///
/// Each agent is spawned as a Docker container. The scheduler maintains an
/// in-memory registry of [`AgentHandle`]s backed by real container lifecycle
/// operations (create, start, stop, remove).
///
/// # Concurrency
///
/// The `agents` map is protected by an [`RwLock`] so the scheduler can be
/// shared across async tasks (wrapped in [`Arc`]).
///
/// # Capacity
///
/// `max_agents` is enforced at spawn time; attempts to exceed it return an
/// [`ClawzError::Orchestration`] error.
pub struct BollardScheduler {
    /// Connected Bollard/Docker client. Talks to the local Docker daemon.
    docker: Docker,
    /// In-memory registry of all agents spawned by this scheduler instance.
    /// Keyed by the agent's [`Uuid`] for O(1) lookups.
    agents: Arc<RwLock<HashMap<Uuid, AgentHandle>>>,
    /// Docker network mode (e.g. `"bridge"`) attached to every spawned container.
    network: String,
    /// Upper bound on the number of concurrent agents this scheduler may run.
    max_agents: usize,
}

impl BollardScheduler {
    /// Create a new `BollardScheduler` connected to the local Docker daemon.
    ///
    /// Defaults to the `"bridge"` network. If your deployment requires a custom
    /// overlay or internal network, use [`Self::with_network`].
    ///
    /// # Errors
    ///
    /// Returns [`ClawzError::Orchestration`] if the Docker daemon is unreachable.
    pub fn new(max_agents: usize) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ClawzError::Orchestration(format!("Docker connect failed: {e}")))?;

        Ok(Self {
            docker,
            agents: Arc::new(RwLock::new(HashMap::new())),
            network: "bridge".into(),
            max_agents,
        })
    }

    /// Create a scheduler with a custom Docker network.
    ///
    /// # Errors
    ///
    /// Same as [`Self::new`] — fails when the Docker daemon is unavailable.
    pub fn with_network(max_agents: usize, network: String) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ClawzError::Orchestration(format!("Docker connect failed: {e}")))?;

        Ok(Self {
            docker,
            agents: Arc::new(RwLock::new(HashMap::new())),
            network,
            max_agents,
        })
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

    /// Build a deterministic container name from tenant and agent IDs.
    ///
    /// Format: `clawz-agent-{tenant_id}-{agent_id}`
    fn container_name(&self, tenant_id: &str, agent_id: &str) -> String {
        format!("clawz-agent-{tenant_id}-{agent_id}")
    }

    /// Build Docker labels applied to every agent container.
    ///
    /// Labels are used for filtering and attribution in external tooling
    /// (e.g. `docker ps --filter label=managed-by=clawz`).
    fn build_labels(
        &self,
        tenant_id: &str,
        agent_id: &str,
        parent_id: Option<&String>,
    ) -> HashMap<String, String> {
        let mut labels = HashMap::new();
        labels.insert("managed-by".to_string(), "clawz".to_string());
        labels.insert("clawz-scheduler".to_string(), "bollard".to_string());
        labels.insert("tenant-id".to_string(), tenant_id.to_string());
        labels.insert("agent-id".to_string(), agent_id.to_string());
        if let Some(p) = parent_id {
            labels.insert("parent-id".to_string(), p.to_string());
        }
        labels
    }

    /// Map a Bollard container state to our domain [`ContainerState`].
    ///
    /// Bollard's status enum is richer than our internal model, so this collapses
    /// similar states:
    ///
    /// | Bollard status | [`ContainerState`] |
    /// |----------------|--------------------|
    /// | RUNNING        | `Running`          |
    /// | EXITED / DEAD  | `Failed`           |
    /// | CREATED / PAUSED / RESTARTING | `Starting` |
    /// | REMOVING       | `Stopping`         |
    /// | _              | `Stopped`          |
    fn map_container_state(
        docker_state: Option<bollard::models::ContainerState>,
    ) -> ContainerState {
        match docker_state.and_then(|s| s.status) {
            Some(bollard::models::ContainerStateStatusEnum::RUNNING) => ContainerState::Running,
            Some(bollard::models::ContainerStateStatusEnum::EXITED)
            | Some(bollard::models::ContainerStateStatusEnum::DEAD) => ContainerState::Failed,
            Some(bollard::models::ContainerStateStatusEnum::CREATED)
            | Some(bollard::models::ContainerStateStatusEnum::PAUSED)
            | Some(bollard::models::ContainerStateStatusEnum::RESTARTING) => {
                ContainerState::Starting
            }
            Some(bollard::models::ContainerStateStatusEnum::REMOVING) => ContainerState::Stopping,
            _ => ContainerState::Stopped,
        }
    }

    /// Pull an image from the registry if it is not already present locally.
    ///
    /// # Why stream consumption matters
    ///
    /// Bollard returns a stream of JSON progress objects. We must consume it
    /// fully (with `StreamExt::next`) before the image is actually cached.
    /// Dropping the stream early may leave the pull in an incomplete state.
    async fn ensure_image(&self, image: &str) -> Result<()> {
        match self.docker.inspect_image(image).await {
            // Image already present — nothing to do.
            Ok(_) => Ok(()),
            Err(_) => {
                let mut stream = self.docker.create_image(
                    Some(CreateImageOptions {
                        from_image: image,
                        ..Default::default()
                    }),
                    None,
                    None,
                );

                while let Some(output) = stream.next().await {
                    if let Err(e) = output {
                        return Err(ClawzError::Orchestration(format!(
                            "failed to pull image '{image}': {e}"
                        )));
                    }
                }
                Ok(())
            }
        }
    }
}

#[async_trait]
impl AgentScheduler for BollardScheduler {
    /// Spawn a new agent as a Docker container.
    ///
    /// # Steps
    /// 1. Enforce `max_agents` capacity guard.
    /// 2. Ensure the requested image is available locally.
    /// 3. Build container name, labels, and environment variables.
    /// 4. Create and start the container via Bollard.
    /// 5. Register the resulting [`AgentHandle`] in the internal map.
    ///
    /// # Errors
    ///
    /// Returns [`ClawzError::Orchestration`] on capacity exhaustion, image pull
    /// failure, or Docker create / start errors.
    async fn spawn_agent(&self, ctx: &TenantContext, spec: AgentSpec) -> Result<AgentHandle> {
        // Capacity guard: acquire a read lock first because contention is lower
        // than a write lock, and most spawn calls will succeed.
        {
            let agents = self.agents.read().unwrap();
            if agents.len() >= self.max_agents {
                return Err(ClawzError::Orchestration(format!(
                    "max agent capacity reached ({})",
                    self.max_agents
                )));
            }
        }

        let agent_id = format!("agent-{}", Uuid::new_v4());
        // 127.0.0.1 is used as a placeholder mesh IP because agent-to-agent
        // communication inside a single Docker host typically routes through
        // localhost port mappings or the shared bridge network.
        let mesh_ip = "127.0.0.1".to_string();

        let mut handle = AgentHandle::new(ctx.tenant_id.clone(), agent_id.clone(), mesh_ip);
        handle.capabilities = spec.capabilities.clone();
        handle.state = ContainerState::Starting;

        // Dependency: clawz_core::types::orchestration::AgentSpec
        self.ensure_image(&spec.image).await?;

        let name = self.container_name(ctx.tenant_id.as_str(), &agent_id);
        let labels = self.build_labels(ctx.tenant_id.as_str(), &agent_id, spec.parent_id.as_ref());

        // Pass runtime configuration into the container as env vars so the
        // agent process can self-identify and connect to the mesh.
        let mut env: Vec<String> = vec![
            format!("CLAWZ_AGENT_ID={agent_id}"),
            format!("CLAWZ_TENANT_ID={}", ctx.tenant_id),
            format!("CLAWZ_MESH_IP=127.0.0.1"),
            format!("CLAWZ_MAX_TOOLS={}", spec.max_tools),
        ];
        if let Ok(url) =
            std::env::var("CLAWZ_PUBLIC_URL").or_else(|_| std::env::var("CLAWZ_GATEWAY_URL"))
        {
            env.push(format!("CLAWZ_GATEWAY_URL={url}"));
            env.push(format!("CLAWZ_PUBLIC_URL={url}"));
        }
        if let Ok(url) = std::env::var("WORKER_URL") {
            env.push(format!("WORKER_URL={url}"));
        }

        // Translate abstract resource limits from AgentSpec into Docker
        // HostConfig fields. Memory is expressed in bytes; CPU in nano-CPUs.
        let host_config = HostConfig {
            memory: Some((spec.memory_mb * 1024 * 1024) as i64),
            memory_swap: Some((spec.memory_mb * 1024 * 1024) as i64),
            nano_cpus: Some((spec.cpu_millicores * 1_000_000) as i64),
            pids_limit: Some(256),
            network_mode: Some(self.network.clone()),
            ..Default::default()
        };

        let container_config = ContainerConfig {
            image: Some(spec.image.as_str()),
            env: Some(env.iter().map(|s| s.as_str()).collect()),
            labels: Some(
                labels
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect(),
            ),
            host_config: Some(host_config),
            ..Default::default()
        };

        let create_opts = CreateContainerOptions {
            name: name.clone(),
            platform: None,
        };

        let response = self
            .docker
            .create_container(Some(create_opts), container_config)
            .await
            .map_err(|e| {
                ClawzError::Orchestration(format!(
                    "failed to create container for agent {agent_id}: {e}"
                ))
            })?;

        let container_id = response.id;

        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| {
                ClawzError::Orchestration(format!(
                    "failed to start container for agent {agent_id}: {e}"
                ))
            })?;

        handle.container_id = Some(container_id);
        handle.state = ContainerState::Ready;

        let mut agents = self.agents.write().unwrap();
        agents.insert(handle.id, handle.clone());

        Ok(handle)
    }

    /// Gracefully stop and remove the container backing `handle`, then
    /// unregister the agent.
    ///
    /// # Why `.ok()` is used on stop / remove
    ///
    /// The container may already be stopped or removed by an external operator
    /// (e.g. `docker rm`). Treating those errors as non-fatal keeps the
    /// scheduler resilient while still failing if the agent is unknown.
    async fn reap_agent(&self, handle: &AgentHandle) -> Result<()> {
        // Verify agent exists and mark as stopping before issuing Docker calls.
        // This prevents races where another task might try to interact with the
        // same agent during shutdown.
        {
            let mut agents = self.agents.write().unwrap();
            match agents.get_mut(&handle.id) {
                Some(h) => h.state = ContainerState::Stopping,
                None => {
                    return Err(ClawzError::NotFound {
                        entity: "agent".into(),
                        id: handle.id.to_string(),
                    });
                }
            }
        }

        let container_id = handle
            .container_id
            .as_ref()
            .ok_or_else(|| ClawzError::Orchestration("agent has no container_id".into()))?;

        // Stop container (ignore already stopped)
        self.docker
            .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
            .await
            .ok();

        // Remove container — force=true ensures removal even if still running.
        self.docker
            .remove_container(
                container_id,
                Some(RemoveContainerOptions {
                    v: true,
                    force: true,
                    link: false,
                }),
            )
            .await
            .ok();

        // Remove from registry
        let mut agents = self.agents.write().unwrap();
        agents.remove(&handle.id);

        Ok(())
    }

    /// Find a "warm" (already running) agent that satisfies *all* requested
    /// capabilities for the given tenant.
    ///
    /// Returns `None` when no alive agent matches, signalling the caller to
    /// spawn a new one.
    async fn find_warm(&self, ctx: &TenantContext, capabilities: &[String]) -> Option<AgentHandle> {
        let agents = self.agents.read().unwrap();
        for (_, handle) in agents.iter() {
            // Tenant isolation: never reuse an agent across tenants.
            if handle.tenant_id != ctx.tenant_id {
                continue;
            }
            // Only consider agents that are still alive.
            if !handle.is_alive() {
                continue;
            }
            // Subset check: the agent must offer every requested capability.
            let has_all = capabilities
                .iter()
                .all(|cap| handle.capabilities.contains(cap));
            if has_all {
                return Some(handle.clone());
            }
        }
        None
    }

    /// List all agents belonging to `tenant_id`.
    async fn list_agents(&self, tenant_id: &str) -> Result<Vec<AgentHandle>> {
        let agents = self.agents.read().unwrap();
        let list: Vec<AgentHandle> = agents
            .values()
            .filter(|h| h.tenant_id.as_str() == tenant_id)
            .cloned()
            .collect();
        Ok(list)
    }

    /// Inspect the Docker container and translate its state into a
    /// [`HealthStatus`] snapshot.
    ///
    /// # Synthetic values
    ///
    /// Memory usage is reported as `0.0` because Bollard 0.18's inspect
    /// endpoint does not expose live memory metrics; real deployments should
    /// augment this with `stats()` calls or cAdvisor.
    async fn health(&self, handle: &AgentHandle) -> Result<HealthStatus> {
        let container_id = handle
            .container_id
            .as_ref()
            .ok_or_else(|| ClawzError::Orchestration("agent has no container_id".into()))?;

        let info = self
            .docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
            .map_err(|_e| ClawzError::NotFound {
                entity: "agent container".into(),
                id: container_id.clone(),
            })?;

        let docker_state = info.state;
        let container_state = Self::map_container_state(docker_state.clone());

        let readiness = match container_state {
            ContainerState::Ready | ContainerState::Running => 1.0,
            ContainerState::Draining => 0.5,
            _ => 0.0,
        };

        let liveness = matches!(
            container_state,
            ContainerState::Ready | ContainerState::Running | ContainerState::Draining
        );

        // Memory usage is not directly available via inspect in bollard 0.18
        let memory_usage_percent = 0.0;

        Ok(HealthStatus {
            readiness,
            liveness,
            // Placeholder: tool slot tracking would require agent-side reporting.
            tool_slots_available: 5,
            memory_usage_percent,
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
        let mut spec = AgentSpec::default();
        spec.image = "alpine:latest".to_string();
        spec.capabilities = vec!["chat".to_string(), "search".to_string()];
        spec
    }

    /// Helper that skips Docker-dependent tests when the daemon is unavailable.
    fn scheduler_or_skip() -> Option<BollardScheduler> {
        match BollardScheduler::new(5) {
            Ok(s) => Some(s),
            Err(_) => {
                eprintln!("Skipping Docker tests — Docker not available");
                None
            }
        }
    }

    #[tokio::test]
    async fn test_bollard_scheduler_constructor() {
        let scheduler = BollardScheduler::new(100);
        assert!(scheduler.is_ok());
        let s = scheduler.unwrap();
        assert_eq!(s.max_agents(), 100);
        assert_eq!(s.agent_count(), 0);
    }

    #[tokio::test]
    async fn test_spawn_agent() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let ctx = test_context();
        let mut spec = test_spec();
        spec.image = "alpine:latest".to_string();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();
        assert_eq!(handle.tenant_id.as_str(), "test-tenant");
        assert!(handle.container_id.is_some());
        assert_eq!(scheduler.agent_count(), 1);

        // Cleanup
        scheduler.reap_agent(&handle).await.ok();
    }

    #[tokio::test]
    async fn test_reap_agent() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let ctx = test_context();
        let mut spec = test_spec();
        spec.image = "alpine:latest".to_string();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();
        assert_eq!(scheduler.agent_count(), 1);

        scheduler.reap_agent(&handle).await.unwrap();
        assert_eq!(scheduler.agent_count(), 0);

        // Reaping again should fail (agent not in registry)
        let result = scheduler.reap_agent(&handle).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_max_agents_capacity() {
        let scheduler = match BollardScheduler::new(2) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("Skipping Docker tests — Docker not available");
                return;
            }
        };
        let ctx = test_context();
        let mut spec = AgentSpec::default();
        spec.image = "alpine:latest".to_string();

        scheduler.spawn_agent(&ctx, spec.clone()).await.unwrap();
        scheduler.spawn_agent(&ctx, spec.clone()).await.unwrap();

        let result = scheduler.spawn_agent(&ctx, spec).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ClawzError::Orchestration(_)));
        assert_eq!(scheduler.agent_count(), 2);

        // Cleanup
        let agents = scheduler.list_agents("test-tenant").await.unwrap();
        for h in agents {
            scheduler.reap_agent(&h).await.ok();
        }
    }

    #[tokio::test]
    async fn test_find_warm() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let ctx = test_context();
        let mut spec = test_spec();
        spec.image = "alpine:latest".to_string();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();

        let found = scheduler.find_warm(&ctx, &["chat".to_string()]).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, handle.id);

        // Cleanup
        scheduler.reap_agent(&handle).await.ok();
    }

    #[tokio::test]
    async fn test_find_warm_missing_capability() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let ctx = test_context();
        let mut spec = test_spec();
        spec.image = "alpine:latest".to_string();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();

        let found = scheduler.find_warm(&ctx, &["missing".to_string()]).await;
        assert!(found.is_none());

        // Cleanup
        scheduler.reap_agent(&handle).await.ok();
    }

    #[tokio::test]
    async fn test_list_agents() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let ctx1 = TenantContext::new(TenantId::new("tenant-1"), Role::Operator);
        let ctx2 = TenantContext::new(TenantId::new("tenant-2"), Role::Operator);
        let mut spec = AgentSpec::default();
        spec.image = "alpine:latest".to_string();

        scheduler.spawn_agent(&ctx1, spec.clone()).await.unwrap();
        scheduler.spawn_agent(&ctx1, spec.clone()).await.unwrap();
        scheduler.spawn_agent(&ctx2, spec.clone()).await.unwrap();

        let list = scheduler.list_agents("tenant-1").await.unwrap();
        assert_eq!(list.len(), 2);

        let list = scheduler.list_agents("tenant-2").await.unwrap();
        assert_eq!(list.len(), 1);

        let list = scheduler.list_agents("tenant-3").await.unwrap();
        assert_eq!(list.len(), 0);

        // Cleanup
        for tenant in ["tenant-1", "tenant-2"] {
            let all = scheduler.list_agents(tenant).await.unwrap();
            for h in all {
                scheduler.reap_agent(&h).await.ok();
            }
        }
    }

    #[tokio::test]
    async fn test_health() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let ctx = test_context();
        let mut spec = AgentSpec::default();
        spec.image = "alpine:latest".to_string();

        let handle = scheduler.spawn_agent(&ctx, spec).await.unwrap();
        let health = scheduler.health(&handle).await.unwrap();

        // Alpine without a command exits immediately, so liveness may be false.
        // readiness should be either 1.0 (running) or 0.0 (other)
        assert!(health.readiness == 1.0 || health.readiness == 0.0);

        // Cleanup
        scheduler.reap_agent(&handle).await.ok();
    }

    #[tokio::test]
    async fn test_health_not_found() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let handle = AgentHandle::new(
            TenantId::new("other"),
            "ghost".to_string(),
            "10.0.0.1".to_string(),
        );

        let result = scheduler.health(&handle).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_find_warm_different_tenant() {
        let Some(scheduler) = scheduler_or_skip() else {
            return;
        };
        let ctx1 = TenantContext::new(TenantId::new("tenant-1"), Role::Operator);
        let ctx2 = TenantContext::new(TenantId::new("tenant-2"), Role::Operator);
        let mut spec = AgentSpec::default();
        spec.image = "alpine:latest".to_string();

        scheduler.spawn_agent(&ctx1, spec.clone()).await.unwrap();

        let found = scheduler.find_warm(&ctx2, &[]).await;
        assert!(found.is_none());

        // Cleanup
        let all = scheduler.list_agents("tenant-1").await.unwrap();
        for h in all {
            scheduler.reap_agent(&h).await.ok();
        }
    }

    #[tokio::test]
    async fn test_with_network() {
        let scheduler = BollardScheduler::with_network(5, "bridge".to_string());
        assert!(scheduler.is_ok());
        let s = scheduler.unwrap();
        assert_eq!(s.max_agents(), 5);
    }

    #[tokio::test]
    async fn test_concurrent_spawn() {
        let scheduler = match BollardScheduler::new(10) {
            Ok(s) => Arc::new(s),
            Err(_) => {
                eprintln!("Skipping Docker tests — Docker not available");
                return;
            }
        };
        let ctx = test_context();
        let mut spec = AgentSpec::default();
        spec.image = "alpine:latest".to_string();

        let mut handles = vec![];
        for _ in 0..5 {
            let s = scheduler.clone();
            let c = ctx.clone();
            let sp = spec.clone();
            handles.push(tokio::spawn(async move { s.spawn_agent(&c, sp).await }));
        }

        let results = futures_util::future::join_all(handles).await;
        let successes: Vec<_> = results
            .iter()
            .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
            .collect();
        assert_eq!(successes.len(), 5);
        assert_eq!(scheduler.agent_count(), 5);

        // Cleanup
        let agents = scheduler.list_agents("test-tenant").await.unwrap();
        for h in agents {
            scheduler.reap_agent(&h).await.ok();
        }
    }
}
