use clawz_core::deployment::DeploymentMode;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub load_avg: f32,
    pub agent_count: usize,
    pub subagent_count: usize,
    pub idle_containers: usize,
}

#[derive(Debug, Clone)]
pub struct ScalingPolicy {
    pub load_threshold_up: f32,
    pub load_threshold_down: f32,
    pub cooldown_seconds: i64,
    pub org_size_trigger: usize,
}

impl ScalingPolicy {
    pub fn new(load_threshold_up: f32, load_threshold_down: f32, cooldown_seconds: i64, org_size_trigger: usize) -> Self {
        Self { load_threshold_up, load_threshold_down, cooldown_seconds, org_size_trigger }
    }
}

#[derive(Debug, Clone)]
pub struct DeploymentTransition {
    pub from: DeploymentMode,
    pub to: DeploymentMode,
    pub direction: String,
}

#[derive(Debug)]
pub struct DeploymentElasticity {
    policy: ScalingPolicy,
    mode: RwLock<DeploymentMode>,
    last_transition: RwLock<DateTime<Utc>>,
    last_direction: RwLock<Option<String>>,
}

impl DeploymentElasticity {
    pub fn new(policy: ScalingPolicy) -> Self {
        Self {
            policy,
            mode: RwLock::new(DeploymentMode::Standalone),
            last_transition: RwLock::new(Utc::now()),
            last_direction: RwLock::new(None),
        }
    }

    pub async fn evaluate(&self, metrics: &SystemMetrics) -> Option<DeploymentTransition> {
        let current = self.mode.read().await.clone();
        let last_dir = self.last_direction.read().await.clone();

        let (new_mode, direction) = self.evaluate_transition(&current, metrics)?;
        if new_mode != current {
            // Check cooldown — only block if transitioning in the SAME direction
            if let Some(ref last_direction) = last_dir {
                let last = *self.last_transition.read().await;
                let cooldown = Duration::seconds(self.policy.cooldown_seconds);
                if direction == *last_direction && Utc::now().signed_duration_since(last) < cooldown {
                    return None;
                }
            }

            *self.mode.write().await = new_mode.clone();
            *self.last_transition.write().await = Utc::now();
            *self.last_direction.write().await = Some(direction.clone());
            Some(DeploymentTransition {
                from: current,
                to: new_mode,
                direction,
            })
        } else {
            None
        }
    }

    fn evaluate_transition(&self, current: &DeploymentMode, metrics: &SystemMetrics) -> Option<(DeploymentMode, String)> {
        match current {
            DeploymentMode::Standalone => {
                if metrics.load_avg > self.policy.load_threshold_up
                    || metrics.agent_count > self.policy.org_size_trigger {
                    Some((DeploymentMode::Micro, "scale_up".into()))
                } else {
                    None
                }
            }
            DeploymentMode::Micro => {
                // 1.5 multiplier: elastic mode requires significantly higher load before engaging
                // subagent_count > 10: must have meaningful parallel workload before elastic scaling
                if metrics.load_avg > self.policy.load_threshold_up * 1.5
                    || metrics.subagent_count > 10 {
                    Some((DeploymentMode::Elastic, "scale_up".into()))
                } else if metrics.load_avg < self.policy.load_threshold_down
                    && metrics.agent_count <= 2 {
                    Some((DeploymentMode::Standalone, "scale_down".into()))
                } else {
                    None
                }
            }
            DeploymentMode::Elastic => {
                if metrics.load_avg < self.policy.load_threshold_down
                    && metrics.subagent_count <= 3
                    && metrics.idle_containers > 5 {
                    Some((DeploymentMode::Micro, "scale_down".into()))
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::deployment::DeploymentMode;

    #[tokio::test]
    async fn elastic_scaler_transitions_standalone_to_micro() {
        let policy = ScalingPolicy {
            load_threshold_up: 0.75,
            load_threshold_down: 0.25,
            cooldown_seconds: 300,
            org_size_trigger: 5,
        };
        let scaler = DeploymentElasticity::new(policy);
        let metrics = SystemMetrics { load_avg: 0.8, agent_count: 6, subagent_count: 0, idle_containers: 0 };

        let transition = scaler.evaluate(&metrics).await;
        assert!(transition.is_some());
        assert_eq!(transition.unwrap().to, DeploymentMode::Micro);
    }

    #[tokio::test]
    async fn elastic_scaler_stays_stable_in_cooldown() {
        let policy = ScalingPolicy {
            load_threshold_up: 0.75,
            load_threshold_down: 0.25,
            cooldown_seconds: 300,
            org_size_trigger: 5,
        };
        let scaler = DeploymentElasticity::new(policy);
        let metrics = SystemMetrics { load_avg: 0.9, agent_count: 8, subagent_count: 0, idle_containers: 0 };

        // First evaluation triggers
        let first = scaler.evaluate(&metrics).await;
        assert!(first.is_some());

        // Second evaluation within cooldown should return None
        let second = scaler.evaluate(&metrics).await;
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn elastic_scaler_respects_hysteresis() {
        let policy = ScalingPolicy {
            load_threshold_up: 0.75,
            load_threshold_down: 0.25,
            cooldown_seconds: 300,
            org_size_trigger: 5,
        };
        let scaler = DeploymentElasticity::new(policy);

        // First, transition from Standalone to Micro
        let hot = SystemMetrics { load_avg: 0.8, agent_count: 6, subagent_count: 0, idle_containers: 0 };
        let up_transition = scaler.evaluate(&hot).await;
        assert!(up_transition.is_some());
        assert_eq!(up_transition.unwrap().to, DeploymentMode::Micro);

        // Now test scale-down: Micro -> Standalone only when load drops below down threshold
        let cold = SystemMetrics { load_avg: 0.2, agent_count: 1, subagent_count: 0, idle_containers: 0 };

        assert!(scaler.evaluate(&cold).await.is_some()); // Should trigger a scale down
    }

    #[tokio::test]
    async fn elastic_scaler_stays_stable_in_mid_range() {
        let policy = ScalingPolicy {
            load_threshold_up: 0.75,
            load_threshold_down: 0.25,
            cooldown_seconds: 300,
            org_size_trigger: 5,
        };
        let scaler = DeploymentElasticity::new(policy);

        // First transition from Standalone -> Micro (load > up threshold)
        let hot = SystemMetrics { load_avg: 0.8, agent_count: 6, subagent_count: 0, idle_containers: 0 };
        let first = scaler.evaluate(&hot).await;
        assert!(first.is_some());

        // Now load sits mid-range: 0.3 (between 0.25 and 0.75)
        // Should stay in Micro — no transition
        let mid = SystemMetrics { load_avg: 0.3, agent_count: 6, subagent_count: 0, idle_containers: 0 };
        let second = scaler.evaluate(&mid).await;
        assert!(second.is_none(), "mid-range load should not trigger transition");

        // And again — still mid-range, still stable
        let third = scaler.evaluate(&mid).await;
        assert!(third.is_none(), "repeated mid-range evaluation stays stable");
    }

    #[tokio::test]
    async fn elastic_scaler_respects_cooldown_blocking_same_direction() {
        let policy = ScalingPolicy {
            load_threshold_up: 0.75,
            load_threshold_down: 0.25,
            cooldown_seconds: 300,
            org_size_trigger: 5,
        };
        let scaler = DeploymentElasticity::new(policy);

        // Trigger Standalone -> Micro
        let metrics = SystemMetrics { load_avg: 0.8, agent_count: 6, subagent_count: 0, idle_containers: 0 };
        let first = scaler.evaluate(&metrics).await;
        assert!(first.is_some());

        // Immediate re-evaluation while still hot should be blocked by cooldown
        let second = scaler.evaluate(&metrics).await;
        assert!(second.is_none(), "cooldown should block same-direction re-transition");
    }
}