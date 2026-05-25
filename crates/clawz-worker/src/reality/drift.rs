//! Drift detection — compare predicted vs observed reality.
//!
//! The [`DriftDetector`] emits [`DriftSignal`]s when the observed
//! [`ContextBundle`] deviates from the predicted one.

use clawz_core::types::{ContextBundle, DriftKind, DriftSignal};

/// Detects drift between a predicted and an observed [`ContextBundle`].
#[derive(Debug, Default)]
pub struct DriftDetector;

impl DriftDetector {
    pub fn new() -> Self {
        Self
    }

    /// Compare `predicted` against `observed` and return all drift signals.
    ///
    /// Checks performed:
    /// - resource limits (max / used mismatches)
    /// - tool list changes (name or version)
    /// - system status divergence
    pub fn compare(predicted: &ContextBundle, observed: &ContextBundle) -> Vec<DriftSignal> {
        let mut signals = Vec::new();

        // Check resource limits
        for pred_limit in &predicted.system_inventory.limits {
            let observed_opt = observed
                .system_inventory
                .limits
                .iter()
                .find(|l| l.resource == pred_limit.resource);

            if let Some(obs_limit) = observed_opt {
                if obs_limit.max_value != pred_limit.max_value {
                    signals.push(DriftSignal {
                        kind: DriftKind::Anomaly,
                        detail: format!(
                            "resource '{}' max_value changed: predicted {} observed {}",
                            pred_limit.resource, pred_limit.max_value, obs_limit.max_value
                        ),
                    });
                }
                if obs_limit.used != pred_limit.used {
                    signals.push(DriftSignal {
                        kind: DriftKind::Anomaly,
                        detail: format!(
                            "resource '{}' used changed: predicted {} observed {}",
                            pred_limit.resource, pred_limit.used, obs_limit.used
                        ),
                    });
                }
            } else {
                signals.push(DriftSignal {
                    kind: DriftKind::ChangeNotification,
                    detail: format!("resource '{}' disappeared from inventory", pred_limit.resource),
                });
            }
        }

        for obs_limit in &observed.system_inventory.limits {
            let pred_opt = predicted
                .system_inventory
                .limits
                .iter()
                .find(|l| l.resource == obs_limit.resource);
            if pred_opt.is_none() {
                signals.push(DriftSignal {
                    kind: DriftKind::ChangeNotification,
                    detail: format!(
                        "resource '{}' appeared in inventory",
                        obs_limit.resource
                    ),
                });
            }
        }

        // Check tool list
        for pred_tool in &predicted.system_inventory.tools {
            let obs_opt = observed
                .system_inventory
                .tools
                .iter()
                .find(|t| t.name == pred_tool.name);
            if let Some(obs_tool) = obs_opt {
                if obs_tool.version != pred_tool.version {
                    signals.push(DriftSignal {
                        kind: DriftKind::ChangeNotification,
                        detail: format!(
                            "tool '{}' version changed: predicted {} observed {}",
                            pred_tool.name, pred_tool.version, obs_tool.version
                        ),
                    });
                }
            } else {
                signals.push(DriftSignal {
                    kind: DriftKind::ChangeNotification,
                    detail: format!("tool '{}' disappeared from inventory", pred_tool.name),
                });
            }
        }

        for obs_tool in &observed.system_inventory.tools {
            if !predicted
                .system_inventory
                .tools
                .iter()
                .any(|t| t.name == obs_tool.name)
            {
                signals.push(DriftSignal {
                    kind: DriftKind::ChangeNotification,
                    detail: format!("tool '{}' appeared in inventory", obs_tool.name),
                });
            }
        }

        // Check system status divergence
        if predicted.current_state.system_status != observed.current_state.system_status {
            signals.push(DriftSignal {
                kind: DriftKind::Anomaly,
                detail: format!(
                    "system_status changed: predicted '{}' observed '{}'",
                    predicted.current_state.system_status, observed.current_state.system_status
                ),
            });
        }

        signals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::{CurrentState, ResourceLimit, SystemInventory};

    #[test]
    fn mutating_resource_consumption_emits_anomaly() {
        let predicted = ContextBundle::builder()
            .current_state(CurrentState {
                system_status: "healthy".into(),
                project_progress: "50%".into(),
                resource_consumption: "low".into(),
            })
            .system_inventory(SystemInventory {
                tools: vec![],
                apis: vec![],
                limits: vec![ResourceLimit {
                    resource: "cpu".into(),
                    max_value: 8,
                    used: 2,
                }],
            })
            .build();

        let observed = ContextBundle::builder()
            .current_state(CurrentState {
                system_status: "healthy".into(),
                project_progress: "50%".into(),
                resource_consumption: "critical".into(),
            })
            .system_inventory(SystemInventory {
                tools: vec![],
                apis: vec![],
                limits: vec![ResourceLimit {
                    resource: "cpu".into(),
                    max_value: 8,
                    used: 7,
                }],
            })
            .build();

        let signals = DriftDetector::compare(&predicted, &observed);
        assert!(
            signals.iter().any(|s| matches!(s.kind, DriftKind::Anomaly)),
            "expected at least one Anomaly drift signal, got {:?}",
            signals
        );
    }
}
