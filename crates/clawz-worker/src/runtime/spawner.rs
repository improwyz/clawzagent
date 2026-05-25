use clawz_core::error::ClawzError;

use crate::runtime::team::TeamRole;

#[derive(Debug, Clone, Copy)]
pub enum ScaleDecision {
    ScaleUp,
    ScaleDown,
    Maintain,
}

#[derive(Debug, Clone)]
pub struct ScalePolicy {
    pub scale_up_threshold: f32,    // tasks per member to trigger scale up
    pub scale_down_threshold: f32,   // tasks per member to trigger scale down
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

pub struct AgentTreeSpawner {
    parent_id: String,
    policy: ScalePolicy,
}

impl AgentTreeSpawner {
    pub fn new(parent_id: &str, policy: ScalePolicy) -> Self {
        Self { parent_id: parent_id.to_string(), policy }
    }

    pub async fn evaluate_workload(&self, queue_depth: usize, active_members: usize) -> ScaleDecision {
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

    pub fn max_capacity_for_role(role: TeamRole) -> usize {
        match role {
            TeamRole::Leader => 5,
            TeamRole::Worker => 8,
            TeamRole::Tester => 3,
            TeamRole::Reviewer => 2,
            _ => 2,
        }
    }

    /// Spawn additional child agents based on scale decision.
    /// Returns the number of agents actually spawned.
    pub async fn spawn_children(
        &self,
        decision: ScaleDecision,
        role: TeamRole,
        count: usize,
    ) -> Result<usize, ClawzError> {
        match decision {
            ScaleDecision::ScaleUp => {
                let current = self.current_children(&role).await;
                let max_cap = Self::max_capacity_for_role(role);
                let to_spawn = count.min(max_cap.saturating_sub(current));
                Ok(to_spawn)
            }
            ScaleDecision::ScaleDown | ScaleDecision::Maintain => Ok(0),
        }
    }

    /// Reap (gracefully shut down) idle child agents.
    /// Returns the number of agents reaped.
    pub async fn reap_idle(
        &self,
        _role: TeamRole,
        _idle_threshold_secs: u64,
    ) -> Result<usize, ClawzError> {
        // In a real implementation, this would drain agents with no active tasks
        // and call SubAgentHandle::cancel() on them
        Ok(0)
    }

    /// Get current child count for a role.
    /// In a real implementation, this queries the Team's member registry.
    async fn current_children(&self, _role: &TeamRole) -> usize {
        0  // Placeholder — would query Team::members() in production
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawner_evaluates_workload_and_decides_scale_up() {
        let policy = ScalePolicy {
            scale_up_threshold: 3.0,
            scale_down_threshold: 1.0,
            max_containers: 5,
        };
        let spawner = AgentTreeSpawner::new("cto-agent", policy);
        // Simulate 5 tasks for 1 member -> ratio 5.0 > threshold 3.0 -> ScaleUp
        let decision = spawner.evaluate_workload(5, 1).await;
        assert!(matches!(decision, ScaleDecision::ScaleUp));
    }

    #[tokio::test]
    async fn spawner_decides_scale_down_when_idle() {
        let policy = ScalePolicy {
            scale_up_threshold: 3.0,
            scale_down_threshold: 1.0,
            max_containers: 5,
        };
        let spawner = AgentTreeSpawner::new("cto-agent", policy);
        // 1 task for 4 members -> ratio 0.25 < threshold 1.0 -> ScaleDown
        let decision = spawner.evaluate_workload(1, 4).await;
        assert!(matches!(decision, ScaleDecision::ScaleDown));
    }

    #[tokio::test]
    async fn spawner_respects_max_capacity() {
        let policy = ScalePolicy {
            scale_up_threshold: 1.0,
            scale_down_threshold: 0.5,
            max_containers: 2,
        };
        let spawner = AgentTreeSpawner::new("cto-agent", policy);
        // 10 members already, should not scale up beyond max_containers
        let decision = spawner.evaluate_workload(20, 10).await;
        assert!(matches!(decision, ScaleDecision::Maintain));
    }
}
