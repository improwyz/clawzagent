//! Discovery — build a [`ContextBundle`] from heterogeneous sources.
//!
//! Sources may be static (configuration), dynamic (simulated health probes),
//! or human-provided facts. The builder computes confidence from how many
//! sources successfully contributed.

use clawz_core::types::{
    ActiveConstraints, ContextBundle, CurrentState, HistoricalPattern, OrgStructure,
    PermissionMatrix, SystemInventory,
};

/// A discovery source for the [`RealityModel`] builder.
#[derive(Debug, Clone)]
pub enum DiscoverySource {
    /// Static configuration: system inventory, permissions, org structure,
    /// and active constraints.
    Static {
        inventory: SystemInventory,
        permissions: PermissionMatrix,
        org: OrgStructure,
        constraints: ActiveConstraints,
    },
    /// Dynamic health probe result (simulated for this phase).
    Dynamic { healthy: bool },
    /// Human-provided key/value pairs merged into [`CurrentState`].
    HumanInput(Vec<(String, String)>),
}

/// Builds [`ContextBundle`] snapshots from a set of [`DiscoverySource`]s.
#[derive(Debug, Default)]
pub struct RealityModel {
    sources: Vec<DiscoverySource>,
}

impl RealityModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a discovery source.
    pub fn with_source(mut self, source: DiscoverySource) -> Self {
        self.sources.push(source);
        self
    }

    /// Consume the model and produce a [`ContextBundle`].
    ///
    /// Confidence is computed from source coverage:
    /// - every successfully processed source contributes `1.0 / total_sources`
    /// - a failed dynamic source contributes nothing
    pub fn build(self, tenant_id: impl Into<String>) -> ContextBundle {
        let tenant_id = tenant_id.into();
        let total_sources = self.sources.len().max(1) as f32;
        let mut successful = 0usize;

        let mut inventory = SystemInventory {
            tools: Vec::new(),
            apis: Vec::new(),
            limits: Vec::new(),
        };
        let mut permissions = PermissionMatrix { entries: Vec::new() };
        let mut org = OrgStructure {
            teams: Vec::new(),
            escalation_paths: Vec::new(),
        };
        let mut constraints = ActiveConstraints {
            policies: Vec::new(),
            slas: Vec::new(),
            budgets: Vec::new(),
            regulations: Vec::new(),
        };
        let mut current_state = CurrentState {
            system_status: String::new(),
            project_progress: String::new(),
            resource_consumption: String::new(),
        };
        let mut historical_patterns = Vec::new();

        for source in self.sources {
            let ok = match source {
                DiscoverySource::Static {
                    inventory: inv,
                    permissions: perm,
                    org: o,
                    constraints: c,
                } => {
                    inventory = inv;
                    permissions = perm;
                    org = o;
                    constraints = c;
                    true
                }
                DiscoverySource::Dynamic { healthy } => {
                    if healthy {
                        current_state.system_status = "healthy".into();
                        true
                    } else {
                        current_state.system_status = "degraded".into();
                        false
                    }
                }
                DiscoverySource::HumanInput(facts) => {
                    for (key, value) in facts {
                        match key.as_str() {
                            "system_status" => current_state.system_status = value,
                            "project_progress" => current_state.project_progress = value,
                            "resource_consumption" => current_state.resource_consumption = value,
                            _ => {
                                // Unknown human fact — store as pattern
                                historical_patterns.push(HistoricalPattern {
                                    pattern: key,
                                    outcome: value,
                                    frequency: 1.0,
                                });
                            }
                        }
                    }
                    true
                }
            };
            if ok {
                successful += 1;
            }
        }

        let confidence = successful as f32 / total_sources;

        ContextBundle::builder()
            .tenant_id(tenant_id)
            .system_inventory(inventory)
            .permission_matrix(permissions)
            .org_structure(org)
            .active_constraints(constraints)
            .current_state(current_state)
            .historical_patterns(historical_patterns)
            .confidence(confidence)
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::{ApiDescriptor, ResourceLimit, Team, ToolDescriptor};

    #[test]
    fn build_from_static_sources_produces_valid_bundle() {
        let model = RealityModel::new().with_source(DiscoverySource::Static {
            inventory: SystemInventory {
                tools: vec![ToolDescriptor {
                    name: "bash".into(),
                    version: "1.0".into(),
                }],
                apis: vec![ApiDescriptor {
                    endpoint: "http://localhost:3000".into(),
                    health: "/health".into(),
                }],
                limits: vec![ResourceLimit {
                    resource: "cpu".into(),
                    max_value: 8,
                    used: 2,
                }],
            },
            permissions: PermissionMatrix { entries: vec![] },
            org: OrgStructure {
                teams: vec![Team {
                    name: "ops".into(),
                    members: vec!["alice".into()],
                }],
                escalation_paths: vec![],
            },
            constraints: ActiveConstraints {
                policies: vec!["p1".into()],
                slas: vec![],
                budgets: vec![],
                regulations: vec![],
            },
        });

        let bundle = model.build("tenant-1");
        assert_eq!(bundle.tenant_id, "tenant-1");
        assert_eq!(bundle.system_inventory.tools.len(), 1);
        assert_eq!(bundle.system_inventory.tools[0].name, "bash");
        assert_eq!(bundle.confidence, 1.0);
    }

    #[test]
    fn degraded_source_still_builds_with_lower_confidence() {
        let model = RealityModel::new()
            .with_source(DiscoverySource::Static {
                inventory: SystemInventory {
                    tools: vec![],
                    apis: vec![],
                    limits: vec![],
                },
                permissions: PermissionMatrix { entries: vec![] },
                org: OrgStructure {
                    teams: vec![],
                    escalation_paths: vec![],
                },
                constraints: ActiveConstraints {
                    policies: vec![],
                    slas: vec![],
                    budgets: vec![],
                    regulations: vec![],
                },
            })
            .with_source(DiscoverySource::Dynamic { healthy: false });

        let bundle = model.build("tenant-2");
        assert_eq!(bundle.confidence, 0.5);
        assert_eq!(bundle.current_state.system_status, "degraded");
    }
}
