//! PRISM-G framework model — the six dimensions of enterprise autonomous AI.
//!
//! PRISM-G is the enterprise autonomous-AI framework authored by Sajan V
//! (Founder & CEO, Enterpryz Ventures). ClawZ is its reference implementation.
//! This module is the conceptual spine: it names the six dimensions and tracks
//! how faithfully each is implemented in the current codebase.
//!
//! The six dimensions, in canonical order, spell PRISM-G:
//! **P**urpose, **R**eality, **I**nfrastructure, **S**warm,
//! **M**emory & Metrics, **G**overnance.
//!
//! IMPORTANT: Governance is ONE dimension. It is not the whole framework.
//! The worker's `governance::guardrails` module implements only the runtime
//! enforcement of the G dimension.

use serde::{Deserialize, Serialize};

/// One of the six PRISM-G dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrismDimension {
    /// P — structuring vague goals into machine-readable objectives.
    Purpose,
    /// R — modeling the environment: systems, constraints, state.
    Reality,
    /// I — safe, governed access to tools and capabilities.
    Infrastructure,
    /// S — coordinating a swarm of specialized (containerized) agents.
    Swarm,
    /// M — learning, measurement, and continuous improvement.
    Memory,
    /// G — safety, alignment, and compliance.
    Governance,
}

impl PrismDimension {
    /// All six dimensions in canonical (PRISMG) order.
    pub const ALL: [PrismDimension; 6] = [
        PrismDimension::Purpose,
        PrismDimension::Reality,
        PrismDimension::Infrastructure,
        PrismDimension::Swarm,
        PrismDimension::Memory,
        PrismDimension::Governance,
    ];

    /// The single-letter acronym component (P, R, I, S, M, G).
    pub fn letter(&self) -> char {
        match self {
            PrismDimension::Purpose => 'P',
            PrismDimension::Reality => 'R',
            PrismDimension::Infrastructure => 'I',
            PrismDimension::Swarm => 'S',
            PrismDimension::Memory => 'M',
            PrismDimension::Governance => 'G',
        }
    }

    /// Human-readable dimension title.
    pub fn title(&self) -> &'static str {
        match self {
            PrismDimension::Purpose => "Purpose",
            PrismDimension::Reality => "Reality",
            PrismDimension::Infrastructure => "Infrastructure",
            PrismDimension::Swarm => "Swarm",
            PrismDimension::Memory => "Memory & Metrics",
            PrismDimension::Governance => "Governance",
        }
    }

    /// One-line description of the dimension's concern.
    pub fn description(&self) -> &'static str {
        match self {
            PrismDimension::Purpose => "Structure vague goals into machine-readable objectives.",
            PrismDimension::Reality => {
                "Model the environment: systems, constraints, and live state."
            }
            PrismDimension::Infrastructure => {
                "Bridge planning to execution with safe, governed tools."
            }
            PrismDimension::Swarm => "Coordinate a swarm of specialized, containerized agents.",
            PrismDimension::Memory => "Learn, measure, and improve over time.",
            PrismDimension::Governance => "Keep the system safe, aligned, and compliant.",
        }
    }
}

/// Honest implementation maturity of a dimension in the current codebase.
///
/// Surfaced in docs and the `/prism` introspection so the platform never
/// overclaims completeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionStatus {
    /// Faithfully implemented and wired into the runtime.
    Implemented,
    /// Partially implemented; some framework concepts missing.
    Partial,
    /// Specified but not yet built.
    Planned,
}

use crate::deployment::DeploymentMode;

/// Declares each dimension's owning module and the deployment modes in which a
/// live capability check MUST pass for the dimension to count as `Implemented`.
///
/// This struct carries **no status field**. Status is *derived* by running the
/// registered [`DimensionCapability`] checks (see [`derive_status`]). It is
/// therefore impossible to mark a dimension `Implemented` by editing a table —
/// the exact overclaiming failure mode that this design forbids.
#[derive(Debug, Clone)]
pub struct DimensionInfo {
    pub dimension: PrismDimension,
    /// Crate-relative module path that owns this dimension's implementation.
    pub module: &'static str,
    /// Deployment modes a capability check must pass in for `Implemented`.
    pub required_modes: &'static [DeploymentMode],
}

/// Canonical declaration of each dimension's owning module and required modes.
/// Carries no status — see [`derive_status`].
pub fn dimension_registry() -> [DimensionInfo; 6] {
    use DeploymentMode::*;
    use PrismDimension::*;
    [
        DimensionInfo {
            dimension: Purpose,
            module: "clawz_worker::purpose",
            required_modes: &[Standalone, Micro, Elastic],
        },
        DimensionInfo {
            dimension: Reality,
            module: "clawz_worker::reality",
            required_modes: &[Standalone, Micro, Elastic],
        },
        DimensionInfo {
            dimension: Infrastructure,
            module: "clawz_worker::tools",
            required_modes: &[Standalone, Micro, Elastic],
        },
        DimensionInfo {
            dimension: Swarm,
            module: "clawz_worker::runtime::team",
            required_modes: &[Micro, Elastic],
        },
        DimensionInfo {
            dimension: Memory,
            module: "clawz_worker::memory",
            required_modes: &[Standalone, Micro, Elastic],
        },
        DimensionInfo {
            dimension: Governance,
            module: "clawz_worker::governance",
            required_modes: &[Standalone, Micro, Elastic],
        },
    ]
}

/// A live wiring check proving a dimension is genuinely operational in one mode.
///
/// Each dimension sub-plan registers one of these as its FINAL step. The docs
/// status table and any `/prism` API are computed by running these checks in a
/// real integration environment — never hand-edited. A check that needs Docker
/// (Swarm) or Postgres (Reality/Memory) only passes where that infra exists, so
/// status is honest by construction.
#[async_trait::async_trait]
pub trait DimensionCapability: Send + Sync {
    fn dimension(&self) -> PrismDimension;
    fn mode(&self) -> DeploymentMode;
    /// `Ok(())` iff the dimension is actually wired and operational in `mode`.
    async fn verify(&self) -> crate::Result<()>;
}

/// Derive a dimension's honest status from the set of modes whose capability
/// check passed:
/// - `Implemented`: a check passed in EVERY one of its `required_modes`.
/// - `Partial`:     at least one required mode passed, but not all.
/// - `Planned`:     no required mode passed (or no check registered).
pub fn derive_status(info: &DimensionInfo, passing_modes: &[DeploymentMode]) -> DimensionStatus {
    if info.required_modes.is_empty() {
        return DimensionStatus::Planned;
    }
    let passed = info
        .required_modes
        .iter()
        .filter(|m| passing_modes.contains(m))
        .count();
    if passed == info.required_modes.len() {
        DimensionStatus::Implemented
    } else if passed > 0 {
        DimensionStatus::Partial
    } else {
        DimensionStatus::Planned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::DeploymentMode::*;

    #[test]
    fn registry_covers_all_dimensions_once() {
        let reg = dimension_registry();
        let mut seen: Vec<PrismDimension> = reg.iter().map(|i| i.dimension).collect();
        seen.dedup();
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn status_is_planned_when_no_modes_pass() {
        let gov = &dimension_registry()[5];
        assert_eq!(derive_status(gov, &[]), DimensionStatus::Planned);
    }

    #[test]
    fn status_is_partial_when_some_modes_pass() {
        let gov = &dimension_registry()[5]; // requires Standalone+Micro+Elastic
        assert_eq!(derive_status(gov, &[Standalone]), DimensionStatus::Partial);
    }

    #[test]
    fn status_is_implemented_only_when_all_required_modes_pass() {
        let gov = &dimension_registry()[5];
        assert_eq!(
            derive_status(gov, &[Standalone, Micro, Elastic]),
            DimensionStatus::Implemented
        );
    }
}
