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
            TeamRole::Leader => 2,
            TeamRole::Reviewer => 2,
            TeamRole::Tester => 3,
            TeamRole::Documenter => 4,
            TeamRole::Auditor => 3,
            TeamRole::Coordinator => 3,
            TeamRole::Worker => 8,
        }
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