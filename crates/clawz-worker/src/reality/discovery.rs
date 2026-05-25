//! Discovery — build a [`ContextBundle`] from heterogeneous sources.
//!
//! Sources may be static (configuration), dynamic (simulated health probes),
//! or human-provided facts. The builder computes confidence from how many
//! sources successfully contributed.

use clawz_core::types::{
    ActiveConstraints, ContextBundle, CurrentState, HistoricalPattern, OrgStructure,
    PermissionMatrix, Reliability, ResourceLimit, SystemInventory,
};

use super::container_metrics::ContainerMetrics;

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
    /// Live container-level telemetry sourced from the Docker (Bollard) API.
    /// Contributes real CPU / memory / health to the resulting
    /// [`ContextBundle`] and bumps confidence to [`Reliability::High`] when
    /// the metrics snapshot reports live data.
    ContainerMetrics(ContainerMetrics),
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
    /// - a [`DiscoverySource::ContainerMetrics`] whose snapshot is live
    ///   (`ContainerMetrics::is_live`) elevates the bundle to
    ///   [`Reliability::High`].
    pub fn build(self, tenant_id: impl Into<String>) -> ContextBundle {
        let tenant_id = tenant_id.into();
        let total_sources = self.sources.len().max(1) as f32;
        let mut successful = 0usize;
        let mut has_live_container_metrics = false;

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
                DiscoverySource::ContainerMetrics(metrics) => {
                    // SystemInventory — surface memory limit as a ResourceLimit
                    // and use the container health field as a quick proxy for
                    // whether tooling is reachable.
                    if metrics.memory_limit_mb > 0 {
                        inventory.limits.push(ResourceLimit {
                            resource: format!("container.memory.{}", metrics.container_id),
                            max_value: metrics.memory_limit_mb,
                            used: metrics.memory_used_mb,
                        });
                    }

                    // CurrentState — record live CPU / memory utilisation and
                    // the container health flag so downstream planners can
                    // adapt without re-polling Docker.
                    current_state.system_status = metrics.health_status.clone();
                    current_state.resource_consumption = format!(
                        "cpu={:.1}% mem={:.1}% queue_depth={}",
                        metrics.cpu_percent,
                        metrics.memory_percent * 100.0,
                        metrics.queue_depth,
                    );
                    current_state.project_progress = format!(
                        "container {} heartbeat {}s ago",
                        metrics.container_id, metrics.last_heartbeat_secs_ago,
                    );

                    let live = metrics.is_live();
                    if live {
                        has_live_container_metrics = true;
                    }
                    live
                }
            };
            if ok {
                successful += 1;
            }
        }

        let confidence = successful as f32 / total_sources;
        // Live container metrics pin reliability to High; otherwise we keep
        // the builder default (currently Medium).
        let reliability = if has_live_container_metrics {
            Reliability::High
        } else {
            Reliability::Medium
        };

        ContextBundle::builder()
            .tenant_id(tenant_id)
            .system_inventory(inventory)
            .permission_matrix(permissions)
            .org_structure(org)
            .active_constraints(constraints)
            .current_state(current_state)
            .historical_patterns(historical_patterns)
            .confidence(confidence)
            .reliability(reliability)
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

    #[test]
    fn container_metrics_source_populates_state_and_inventory() {
        // Construct a live container-metrics snapshot manually — exercising
        // the build path without requiring a running Docker daemon.
        let metrics = super::ContainerMetrics {
            container_id: "abc123".into(),
            cpu_percent: 42.5,
            memory_percent: 0.6,
            memory_used_mb: 600,
            memory_limit_mb: 1000,
            queue_depth: 3,
            health_status: "healthy".into(),
            last_heartbeat_secs_ago: 7,
            timestamp: chrono::Utc::now(),
        };
        assert!(metrics.is_live());

        let model = RealityModel::new()
            .with_source(DiscoverySource::ContainerMetrics(metrics));
        let bundle = model.build("tenant-live");

        assert_eq!(bundle.current_state.system_status, "healthy");
        assert!(bundle
            .current_state
            .resource_consumption
            .contains("cpu=42.5%"));
        assert!(bundle
            .current_state
            .resource_consumption
            .contains("queue_depth=3"));
        assert_eq!(bundle.system_inventory.limits.len(), 1);
        assert_eq!(bundle.system_inventory.limits[0].max_value, 1000);
        assert_eq!(bundle.system_inventory.limits[0].used, 600);
        assert_eq!(bundle.confidence, 1.0);
        assert_eq!(bundle.reliability, clawz_core::types::Reliability::High);
    }

    #[test]
    fn container_metrics_unavailable_does_not_elevate_reliability() {
        let metrics = super::ContainerMetrics::unavailable("dead-container");
        let model = RealityModel::new()
            .with_source(DiscoverySource::ContainerMetrics(metrics));
        let bundle = model.build("tenant-dead");
        // Unavailable snapshot is not "live" — reliability stays Medium and
        // the source counts as failed (confidence 0).
        assert_eq!(bundle.reliability, clawz_core::types::Reliability::Medium);
        assert_eq!(bundle.confidence, 0.0);
        assert_eq!(bundle.current_state.system_status, "unknown");
    }
}
