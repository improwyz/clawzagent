// tool_orchestrator.rs — ToolOrchestrator trait implementations for Docker and in-memory backends

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bollard::container::{
    Config as ContainerConfig, CreateContainerOptions, InspectContainerOptions,
    RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::HostConfig;
use bollard::Docker;
use chrono::Utc;
use futures_util::StreamExt;
use uuid::Uuid;

use clawz_core::error::{ClawzError, Result};
use clawz_core::traits::ToolOrchestrator;
use clawz_core::types::orchestration::{ContainerState, HealthStatus, SpawnConfig, ToolHandle, ToolType};

// ---------------------------------------------------------------------------
// DockerToolOrchestrator
// ---------------------------------------------------------------------------

/// Docker-backed tool orchestrator using the Bollard Docker API.
pub struct DockerToolOrchestrator {
    docker: Docker,
    tools: Arc<RwLock<HashMap<uuid::Uuid, ToolHandle>>>,
    network: String,
}

impl DockerToolOrchestrator {
    pub fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ClawzError::Orchestration(format!("Docker connect failed: {e}")))?;
        Ok(Self {
            docker,
            tools: Arc::new(RwLock::new(HashMap::new())),
            network: "bridge".into(),
        })
    }

    pub fn with_network(network: String) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ClawzError::Orchestration(format!("Docker connect failed: {e}")))?;
        Ok(Self {
            docker,
            tools: Arc::new(RwLock::new(HashMap::new())),
            network,
        })
    }

    fn build_labels(&self, tool_type: ToolType, owner_agent_id: &str) -> HashMap<String, String> {
        let mut labels = HashMap::new();
        labels.insert("managed-by".to_string(), "clawz".to_string());
        labels.insert("clawz-orchestrator".to_string(), "docker".to_string());
        labels.insert("tool-type".to_string(), tool_type.to_string());
        labels.insert("owner-agent-id".to_string(), owner_agent_id.to_string());
        labels
    }

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
            Some(bollard::models::ContainerStateStatusEnum::REMOVING) => {
                ContainerState::Stopping
            }
            _ => ContainerState::Stopped,
        }
    }

    async fn ensure_image(&self, image: &str) -> Result<()> {
        match self.docker.inspect_image(image).await {
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

    fn image_for_tool_type(tool_type: ToolType) -> String {
        match tool_type {
            ToolType::Browser => "clawz/browser:latest".to_string(),
            ToolType::Sandbox => "clawz/sandbox:latest".to_string(),
            ToolType::McpBridge => "clawz/mcp-bridge:latest".to_string(),
            ToolType::Connectors => "clawz/connectors:latest".to_string(),
            ToolType::Infra => "clawz/infra:latest".to_string(),
        }
    }
}

#[async_trait]
impl ToolOrchestrator for DockerToolOrchestrator {
    async fn spawn_tool(&self, tool_type: ToolType, config: SpawnConfig) -> Result<ToolHandle> {
        let owner_agent_id = config
            .labels
            .iter()
            .find(|(k, _)| k == "owner-agent-id")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "default".to_string());

        let mut handle =
            ToolHandle::new(owner_agent_id.clone(), tool_type, "127.0.0.1".to_string());
        handle.state = ContainerState::Starting;

        let image = if config.image.is_empty() {
            Self::image_for_tool_type(tool_type)
        } else {
            config.image
        };

        self.ensure_image(&image).await?;

        let labels = self.build_labels(tool_type, &owner_agent_id);

        let env: Vec<String> = vec![
            format!("CLAWZ_TOOL_TYPE={}", tool_type),
            format!("CLAWZ_OWNER_AGENT_ID={}", owner_agent_id),
        ];

        let host_config = HostConfig {
            memory: Some((config.memory_mb * 1024 * 1024) as i64),
            memory_swap: Some((config.memory_mb * 1024 * 1024) as i64),
            nano_cpus: Some((config.cpu_millicores * 1_000_000) as i64),
            pids_limit: Some(128),
            network_mode: Some(self.network.clone()),
            ..Default::default()
        };

        let container_config = ContainerConfig {
            image: Some(image.as_str()),
            env: Some(env.iter().map(|s| s.as_str()).collect()),
            labels: Some(labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()),
            host_config: Some(host_config),
            ..Default::default()
        };

        let container_name = format!("clawz-tool-{}-{}", tool_type, Uuid::new_v4());
        let create_opts = CreateContainerOptions {
            name: container_name.clone(),
            platform: None,
        };

        let response = self
            .docker
            .create_container(Some(create_opts), container_config)
            .await
            .map_err(|e| {
                ClawzError::Orchestration(format!(
                    "failed to create container for tool {tool_type}: {e}"
                ))
            })?;

        let container_id = response.id;

        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| {
                ClawzError::Orchestration(format!(
                    "failed to start container for tool {tool_type}: {e}"
                ))
            })?;

        handle.container_id = Some(container_id);
        handle.state = ContainerState::Ready;

        let mut tools = self.tools.write().unwrap();
        tools.insert(handle.id, handle.clone());

        Ok(handle)
    }

    async fn reap_tool(&self, handle: &ToolHandle) -> Result<()> {
        {
            let mut tools = self.tools.write().unwrap();
            match tools.get_mut(&handle.id) {
                Some(h) => h.state = ContainerState::Stopping,
                None => {
                    return Err(ClawzError::NotFound {
                        entity: "tool".into(),
                        id: handle.id.to_string(),
                    });
                }
            }
        }

        let container_id = handle.container_id.as_ref().ok_or_else(|| {
            ClawzError::Orchestration("tool has no container_id".into())
        })?;

        self.docker
            .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
            .await
            .ok();

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

        let mut tools = self.tools.write().unwrap();
        tools.remove(&handle.id);

        Ok(())
    }

    async fn health(&self, handle: &ToolHandle) -> Result<HealthStatus> {
        let container_id = handle.container_id.as_ref().ok_or_else(|| {
            ClawzError::Orchestration("tool has no container_id".into())
        })?;

        let info = self
            .docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
            .map_err(|_| ClawzError::NotFound {
                entity: "tool container".into(),
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

        Ok(HealthStatus {
            readiness,
            liveness,
            tool_slots_available: 3,
            memory_usage_percent: 0.0,
            last_check: Utc::now(),
        })
    }

    async fn list_tools(&self) -> Result<Vec<ToolHandle>> {
        let tools = self.tools.read().unwrap();
        Ok(tools.values().cloned().collect())
    }

    async fn cascade_kill(&self, agent_id: &str, grace_secs: u64) -> Result<u32> {
        // Collect IDs under read lock, then drop immediately
        let to_remove: Vec<(Uuid, Option<String>)> = {
            let tools = self.tools.read().unwrap();
            tools
                .values()
                .filter(|h| h.owner_agent_id.starts_with(agent_id))
                .map(|h| (h.id, h.container_id.clone()))
                .collect()
        };

        // Mark as Stopping while we hold the write lock
        {
            let mut tools = self.tools.write().unwrap();
            for (id, _) in &to_remove {
                if let Some(h) = tools.get_mut(id) {
                    h.state = ContainerState::Stopping;
                }
            }
        }

        // Perform Docker operations outside of any lock
        let mut killed = 0u32;
        for (_, cid_opt) in &to_remove {
            if let Some(cid) = cid_opt {
                self.docker
                    .stop_container(cid, Some(StopContainerOptions { t: grace_secs as i64 }))
                    .await
                    .ok();
                self.docker
                    .remove_container(
                        cid,
                        Some(RemoveContainerOptions {
                            v: true,
                            force: true,
                            link: false,
                        }),
                    )
                    .await
                    .ok();
                killed += 1;
            }
        }

        // Remove from registry
        let mut tools = self.tools.write().unwrap();
        for (id, _) in &to_remove {
            tools.remove(id);
        }

        Ok(killed)
    }
}

// ---------------------------------------------------------------------------
// InMemoryToolOrchestrator
// ---------------------------------------------------------------------------

/// Simple in-process tool orchestrator backed by a HashMap.
/// For use in tests and environments where Docker is not available.
#[derive(Debug, Clone)]
pub struct InMemoryToolOrchestrator {
    tools: Arc<RwLock<HashMap<uuid::Uuid, ToolHandle>>>,
}

impl InMemoryToolOrchestrator {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn a tool synchronously (test helper).
    pub fn spawn_tool_sync(
        &self,
        tool_type: ToolType,
        config: SpawnConfig,
    ) -> Result<ToolHandle> {
        let owner_agent_id = config
            .labels
            .iter()
            .find(|(k, _)| k == "owner-agent-id")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "default".to_string());

        let handle = ToolHandle::new(owner_agent_id, tool_type, "127.0.0.1".to_string());
        let mut tools = self.tools.write().unwrap();
        tools.insert(handle.id, handle.clone());
        Ok(handle)
    }

    /// Reap a tool synchronously (test helper).
    pub fn reap_tool_sync(&self, handle: &ToolHandle) -> Result<()> {
        let mut tools = self.tools.write().unwrap();
        tools.remove(&handle.id);
        Ok(())
    }

    /// List all tools synchronously (test helper).
    pub fn list_tools_sync(&self) -> Result<Vec<ToolHandle>> {
        let tools = self.tools.read().unwrap();
        Ok(tools.values().cloned().collect())
    }

    /// Gracefully stop all tools owned by an agent synchronously (test helper).
    pub fn cascade_kill_sync(&self, agent_id: &str) -> u32 {
        let mut tools = self.tools.write().unwrap();
        let to_remove: Vec<Uuid> = tools
            .iter()
            .filter(|(_, h)| h.owner_agent_id.starts_with(agent_id))
            .map(|(id, _)| *id)
            .collect();
        let count = to_remove.len() as u32;
        for id in to_remove {
            tools.remove(&id);
        }
        count
    }
}

impl Default for InMemoryToolOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolOrchestrator for InMemoryToolOrchestrator {
    async fn spawn_tool(&self, tool_type: ToolType, config: SpawnConfig) -> Result<ToolHandle> {
        let mut handle = self.spawn_tool_sync(tool_type, config)?;
        handle.state = ContainerState::Ready;
        {
            let mut tools = self.tools.write().unwrap();
            tools.insert(handle.id, handle.clone());
        }
        Ok(handle)
    }

    async fn reap_tool(&self, handle: &ToolHandle) -> Result<()> {
        self.reap_tool_sync(handle)
    }

    async fn health(&self, handle: &ToolHandle) -> Result<HealthStatus> {
        let tools = self.tools.read().unwrap();
        let h = tools
            .get(&handle.id)
            .ok_or_else(|| ClawzError::NotFound { entity: "tool".into(), id: handle.id.to_string() })?;

        let liveness = matches!(
            h.state,
            ContainerState::Ready | ContainerState::Running | ContainerState::Draining
        );
        let readiness = if liveness { 1.0 } else { 0.0 };

        Ok(HealthStatus {
            readiness,
            liveness,
            tool_slots_available: 5,
            memory_usage_percent: 0.0,
            last_check: Utc::now(),
        })
    }

    async fn list_tools(&self) -> Result<Vec<ToolHandle>> {
        let tools = self.tools.read().unwrap();
        Ok(tools.values().cloned().collect())
    }

    async fn cascade_kill(&self, agent_id: &str, _grace_secs: u64) -> Result<u32> {
        let mut tools = self.tools.write().unwrap();
        let to_remove: Vec<Uuid> = tools
            .iter()
            .filter(|(_, h)| h.owner_agent_id.starts_with(agent_id))
            .map(|(id, _)| *id)
            .collect();

        let count = to_remove.len() as u32;
        for id in to_remove {
            tools.remove(&id);
        }
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_spawn_config(owner_agent_id: &str) -> SpawnConfig {
        SpawnConfig {
            memory_mb: 256,
            cpu_millicores: 500,
            image: String::new(),
            env: Vec::new(),
            labels: vec![("owner-agent-id".to_string(), owner_agent_id.to_string())],
            network: "bridge".to_string(),
        }
    }

    #[test]
    fn test_in_memory_spawn_reap() {
        let orch = InMemoryToolOrchestrator::new();
        let config = make_spawn_config("agent-1");
        let handle = orch.spawn_tool_sync(ToolType::Browser, config).unwrap();
        // In-memory backend has no container IDs
        assert!(handle.container_id.is_none());
        assert_eq!(handle.owner_agent_id, "agent-1");

        let listed = orch.list_tools_sync().unwrap();
        assert_eq!(listed.len(), 1);

        orch.reap_tool_sync(&handle).unwrap();
        assert_eq!(orch.list_tools_sync().unwrap().len(), 0);
    }

    #[test]
    fn test_in_memory_cascade_kill() {
        let orch = InMemoryToolOrchestrator::new();
        let config1 = make_spawn_config("agent-team-a-worker-1");
        let config2 = make_spawn_config("agent-team-a-worker-2");
        let config3 = make_spawn_config("agent-team-b-worker-1");

        orch.spawn_tool_sync(ToolType::Browser, config1).unwrap();
        orch.spawn_tool_sync(ToolType::Sandbox, config2).unwrap();
        orch.spawn_tool_sync(ToolType::McpBridge, config3).unwrap();

        assert_eq!(orch.list_tools_sync().unwrap().len(), 3);

        let killed = orch.cascade_kill_sync("agent-team-a");
        assert_eq!(killed, 2);
        assert_eq!(orch.list_tools_sync().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_in_memory_tool_orchestrator_async() {
        let orch = InMemoryToolOrchestrator::new();
        let config = make_spawn_config("test-agent");

        let handle = orch.spawn_tool(ToolType::Sandbox, config).await.unwrap();
        let listed = orch.list_tools().await.unwrap();
        assert_eq!(listed.len(), 1);

        orch.reap_tool(&handle).await.unwrap();
        assert_eq!(orch.list_tools().await.unwrap().len(), 0);
    }
}
