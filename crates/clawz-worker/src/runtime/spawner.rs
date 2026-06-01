use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::traits::AgentScheduler;
use clawz_core::types::TenantContext;
use clawz_core::types::orchestration::{AgentHandle, AgentSpec, ContainerState, HealthStatus};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::runtime::complexity::ComplexityScore;
use crate::runtime::team::TeamRole;

// ---------------------------------------------------------------------------
// Scale decision & policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum ScaleDecision {
    ScaleUp,
    ScaleDown,
    Maintain,
}

#[derive(Debug, Clone)]
pub struct ScalePolicy {
    pub scale_up_threshold: f32,   // tasks per member to trigger scale up
    pub scale_down_threshold: f32, // tasks per member to trigger scale down
    pub max_containers: usize,
}

impl ScalePolicy {
    pub fn new(scale_up_threshold: f32, scale_down_threshold: f32, max_containers: usize) -> Self {
        Self {
            scale_up_threshold,
            scale_down_threshold,
            max_containers,
        }
    }
}

// ---------------------------------------------------------------------------
// AgentTreeSpawner
// ---------------------------------------------------------------------------

pub struct AgentTreeSpawner {
    parent_id: String,
    policy: ScalePolicy,
    scheduler: Arc<dyn AgentScheduler>,
    tenant_context: TenantContext,
    default_image: String,
    child_registry: RwLock<HashMap<String, Vec<AgentHandle>>>,
}

impl AgentTreeSpawner {
    pub fn new(
        parent_id: &str,
        policy: ScalePolicy,
        scheduler: Arc<dyn AgentScheduler>,
        tenant_context: TenantContext,
        default_image: &str,
    ) -> Self {
        Self {
            parent_id: parent_id.to_string(),
            policy,
            scheduler,
            tenant_context,
            default_image: default_image.to_string(),
            child_registry: RwLock::new(HashMap::new()),
        }
    }

    pub async fn evaluate_workload(
        &self,
        queue_depth: usize,
        active_members: usize,
    ) -> ScaleDecision {
        if active_members == 0 {
            return ScaleDecision::Maintain;
        }
        let ratio = queue_depth as f32 / active_members as f32;

        if ratio >= self.policy.scale_up_threshold && active_members < self.policy.max_containers {
            ScaleDecision::ScaleUp
        } else if ratio <= self.policy.scale_down_threshold && active_members > 1 {
            ScaleDecision::ScaleDown
        } else {
            ScaleDecision::Maintain
        }
    }

    pub async fn spawn_children(
        &self,
        decision: ScaleDecision,
        role: TeamRole,
        count: usize,
    ) -> Result<Vec<AgentHandle>, ClawzError> {
        // Preserve the original semantic that count == 0 spawns nothing.
        if count == 0 {
            return Ok(vec![]);
        }
        // Delegate to the complexity-aware path with a synthetic score that
        // mirrors the caller's requested `count` exactly.
        let score = ComplexityScore::from_parallelism(count);
        self.spawn_children_with_complexity(decision, role, &score)
            .await
    }

    /// Spawn children sized by a [`ComplexityScore`].
    ///
    /// The number of children actually spawned is:
    ///
    ///   `min(complexity.parallelism_hint, max_capacity_for_role(role) - current_children)`
    ///
    /// On any non-`ScaleUp` decision this returns an empty vec.
    pub async fn spawn_children_with_complexity(
        &self,
        decision: ScaleDecision,
        role: TeamRole,
        complexity: &ComplexityScore,
    ) -> Result<Vec<AgentHandle>, ClawzError> {
        if !matches!(decision, ScaleDecision::ScaleUp) {
            return Ok(vec![]);
        }

        let max_cap = Self::max_capacity_for_role(role);
        let current = self.current_children(&role).await;
        let headroom = max_cap.saturating_sub(current);
        let to_spawn = complexity.parallelism_hint.min(headroom);

        let mut handles = Vec::new();
        for _ in 0..to_spawn {
            let spec = AgentSpec {
                parent_id: Some(self.parent_id.clone()),
                memory_mb: 512,
                cpu_millicores: 1000,
                image: self.default_image.clone(),
                capabilities: vec![role.to_string()],
                max_tools: 10,
                idle_timeout_secs: 300,
            };

            match self.scheduler.spawn_agent(&self.tenant_context, spec).await {
                Ok(handle) => {
                    let mut registry = self.child_registry.write().await;
                    registry
                        .entry(role.to_string())
                        .or_insert_with(Vec::new)
                        .push(handle.clone());
                    handles.push(handle);
                }
                Err(e) => {
                    log::error!("failed to spawn child: {e}");
                }
            }
        }

        Ok(handles)
    }

    pub async fn reap_idle(
        &self,
        role: TeamRole,
        _idle_threshold_secs: u64,
    ) -> Result<usize, ClawzError> {
        let mut registry = self.child_registry.write().await;
        let handles = registry.get_mut(&role.to_string());

        let mut reaped = 0;
        if let Some(h) = handles {
            let to_reap: Vec<_> = h.iter().filter(|h| h.state.is_idle()).cloned().collect();
            for handle in to_reap {
                if self.scheduler.reap_agent(&handle).await.is_ok() {
                    reaped += 1;
                }
            }
            h.retain(|h| !h.state.is_idle());
        }

        Ok(reaped)
    }

    async fn current_children(&self, role: &TeamRole) -> usize {
        let registry = self.child_registry.read().await;
        registry
            .get(&role.to_string())
            .map(|h| h.len())
            .unwrap_or(0)
    }

    pub fn max_capacity_for_role(role: TeamRole) -> usize {
        match role {
            TeamRole::Leader => 5,
            TeamRole::Worker => 8,
            TeamRole::Tester => 3,
            TeamRole::Reviewer => 2,
            _ => 2,
        }
    }
}

// ---------------------------------------------------------------------------
// Mock scheduler for tests
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct MockScheduler {
    spawned: Arc<std::sync::Mutex<Vec<AgentHandle>>>,
}

impl MockScheduler {
    #[allow(dead_code)]
    fn new() -> Self {
        Self {
            spawned: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl AgentScheduler for MockScheduler {
    async fn spawn_agent(
        &self,
        _ctx: &TenantContext,
        spec: AgentSpec,
    ) -> Result<AgentHandle, ClawzError> {
        let handle = AgentHandle {
            id: uuid::Uuid::new_v4(),
            tenant_id: spec.parent_id.clone().unwrap_or_default().into(),
            agent_id: format!("child-{}", uuid::Uuid::new_v4()),
            container_id: Some(format!("container-{}", uuid::Uuid::new_v4())),
            mesh_ip: "127.0.0.1".to_string(),
            state: ContainerState::Starting,
            capabilities: spec.capabilities,
            spawned_at: chrono::Utc::now(),
            last_heartbeat: chrono::Utc::now(),
        };
        self.spawned.lock().unwrap().push(handle.clone());
        Ok(handle)
    }

    async fn reap_agent(&self, _handle: &AgentHandle) -> Result<(), ClawzError> {
        Ok(())
    }

    async fn find_warm(&self, _ctx: &TenantContext, _caps: &[String]) -> Option<AgentHandle> {
        None
    }

    async fn list_agents(&self, _tenant_id: &str) -> Result<Vec<AgentHandle>, ClawzError> {
        Ok(vec![])
    }

    async fn health(&self, _handle: &AgentHandle) -> Result<HealthStatus, ClawzError> {
        Ok(HealthStatus {
            readiness: 1.0,
            liveness: true,
            tool_slots_available: 5,
            memory_usage_percent: 0.0,
            last_check: chrono::Utc::now(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tenant() -> TenantContext {
        TenantContext::new("test-tenant".into(), clawz_core::types::TenantRole::Agent)
    }

    #[tokio::test]
    async fn spawner_evaluates_workload_and_decides_scale_up() {
        let policy = ScalePolicy::new(3.0, 1.0, 5);
        // Use a dummy scheduler — evaluate_workload doesn't need real spawning
        let scheduler = Arc::new(MockScheduler::new()) as Arc<dyn AgentScheduler>;
        let spawner = AgentTreeSpawner::new(
            "cto-agent",
            policy,
            scheduler,
            make_tenant(),
            "clawz/agent:latest",
        );
        // 5 tasks for 1 member -> ratio 5.0 >= threshold 3.0 -> ScaleUp
        let decision = spawner.evaluate_workload(5, 1).await;
        assert!(matches!(decision, ScaleDecision::ScaleUp));
    }

    #[tokio::test]
    async fn spawner_decides_scale_down_when_idle() {
        let policy = ScalePolicy::new(3.0, 1.0, 5);
        let scheduler = Arc::new(MockScheduler::new()) as Arc<dyn AgentScheduler>;
        let spawner = AgentTreeSpawner::new(
            "cto-agent",
            policy,
            scheduler,
            make_tenant(),
            "clawz/agent:latest",
        );
        // 1 task for 4 members -> ratio 0.25 <= threshold 1.0 -> ScaleDown
        let decision = spawner.evaluate_workload(1, 4).await;
        assert!(matches!(decision, ScaleDecision::ScaleDown));
    }

    #[tokio::test]
    async fn spawner_spawn_children_actually_creates_agents() {
        let policy = ScalePolicy::new(1.0, 0.5, 10);
        let mock = MockScheduler::new();
        let spawned = mock.spawned.clone();
        let scheduler = Arc::new(mock) as Arc<dyn AgentScheduler>;
        let tenant_ctx = make_tenant();
        let spawner = AgentTreeSpawner::new(
            "parent-agent",
            policy,
            scheduler,
            tenant_ctx,
            "clawz/agent:latest",
        );

        let handles = spawner
            .spawn_children(ScaleDecision::ScaleUp, TeamRole::Worker, 3)
            .await
            .unwrap();
        assert_eq!(handles.len(), 3);

        let all_spawned = spawned.lock().unwrap();
        assert_eq!(all_spawned.len(), 3);
        for h in all_spawned.iter() {
            assert_eq!(h.tenant_id.as_str(), "parent-agent");
            assert!(h.capabilities.contains(&"worker".to_string()));
        }
    }

    #[tokio::test]
    async fn spawner_does_not_spawn_on_maintain() {
        let policy = ScalePolicy::new(1.0, 0.5, 10);
        let scheduler = Arc::new(MockScheduler::new()) as Arc<dyn AgentScheduler>;
        let spawner = AgentTreeSpawner::new(
            "cto-agent",
            policy,
            scheduler,
            make_tenant(),
            "clawz/agent:latest",
        );

        let handles = spawner
            .spawn_children(ScaleDecision::Maintain, TeamRole::Worker, 5)
            .await
            .unwrap();
        assert!(handles.is_empty());
    }

    #[tokio::test]
    async fn spawner_with_complexity_honors_parallelism_hint() {
        use crate::runtime::complexity::ComplexityScore;

        let policy = ScalePolicy::new(1.0, 0.5, 10);
        let scheduler = Arc::new(MockScheduler::new()) as Arc<dyn AgentScheduler>;
        let spawner = AgentTreeSpawner::new(
            "parent",
            policy,
            scheduler,
            make_tenant(),
            "clawz/agent:latest",
        );

        // parallelism_hint = 4 -> spawn 4 workers (worker max cap = 8).
        let score = ComplexityScore::from_parallelism(4);
        let handles = spawner
            .spawn_children_with_complexity(ScaleDecision::ScaleUp, TeamRole::Worker, &score)
            .await
            .unwrap();
        assert_eq!(handles.len(), 4);
    }

    #[tokio::test]
    async fn spawner_with_complexity_caps_at_role_capacity() {
        use crate::runtime::complexity::ComplexityScore;

        let policy = ScalePolicy::new(1.0, 0.5, 10);
        let scheduler = Arc::new(MockScheduler::new()) as Arc<dyn AgentScheduler>;
        let spawner = AgentTreeSpawner::new(
            "parent",
            policy,
            scheduler,
            make_tenant(),
            "clawz/agent:latest",
        );

        // parallelism = 8 (Massive) but Reviewer cap = 2 -> only 2 spawn.
        let score = ComplexityScore::from_parallelism(8);
        let handles = spawner
            .spawn_children_with_complexity(ScaleDecision::ScaleUp, TeamRole::Reviewer, &score)
            .await
            .unwrap();
        assert_eq!(handles.len(), 2);
    }

    #[tokio::test]
    async fn spawner_with_complexity_ignores_non_scaleup_decisions() {
        use crate::runtime::complexity::ComplexityScore;

        let policy = ScalePolicy::new(1.0, 0.5, 10);
        let scheduler = Arc::new(MockScheduler::new()) as Arc<dyn AgentScheduler>;
        let spawner = AgentTreeSpawner::new(
            "parent",
            policy,
            scheduler,
            make_tenant(),
            "clawz/agent:latest",
        );

        let score = ComplexityScore::from_parallelism(5);
        let handles = spawner
            .spawn_children_with_complexity(ScaleDecision::ScaleDown, TeamRole::Worker, &score)
            .await
            .unwrap();
        assert!(handles.is_empty());
    }
}
