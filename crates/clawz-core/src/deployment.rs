//! Deployment topology enumeration for the ClawZ platform.
//!
//! Determines whether the node runs as a single monolith, a set of
//! containerised microservices, or an elastic mesh of agents.
//!
//! // Dependency: read by worker::bootstrap and gateway::bootstrap on startup.

use serde::{Deserialize, Serialize};

/// Topology mode selected at runtime via the `CLAWZ_MODE` environment variable.
/// // Used by: worker::scheduler, gateway::server
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentMode {
    /// Single-process monolith — everything in one binary.
    #[default]
    Standalone,
    /// Container-per-service — worker and gateway run in separate containers.
    Micro,
    /// Elastic agent mesh — workers auto-scale and form a WireGuard mesh.
    Elastic,
}

impl DeploymentMode {
    /// Read the mode from `CLAWZ_MODE`. Falls back to `Standalone` if the
    /// env var is missing or unrecognised so the system is always runnable.
    pub fn from_env() -> Self {
        match std::env::var("CLAWZ_MODE").as_deref() {
            Ok("standalone") => Self::Standalone,
            Ok("micro") => Self::Micro,
            Ok("elastic") => Self::Elastic,
            _ => Self::Standalone,
        }
    }

    /// Returns true when the runtime should spin up Docker/OCI containers
    /// for agents and tools.
    pub fn uses_containers(&self) -> bool {
        matches!(self, Self::Micro | Self::Elastic)
    }

    /// Returns true when the runtime should join the WireGuard mesh
    /// and enable peer-to-peer agent routing.
    pub fn uses_mesh(&self) -> bool {
        matches!(self, Self::Elastic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_standalone() {
        assert_eq!(DeploymentMode::default(), DeploymentMode::Standalone);
    }

    #[test]
    fn uses_containers() {
        assert!(!DeploymentMode::Standalone.uses_containers());
        assert!(DeploymentMode::Micro.uses_containers());
        assert!(DeploymentMode::Elastic.uses_containers());
    }

    #[test]
    fn uses_mesh() {
        assert!(!DeploymentMode::Standalone.uses_mesh());
        assert!(!DeploymentMode::Micro.uses_mesh());
        assert!(DeploymentMode::Elastic.uses_mesh());
    }
}
