//! Scheduler factory — selects the concrete [`AgentScheduler`] implementation
//! based on the active [`DeploymentMode`].
//!
//! This module provides a single entry-point, [`create_scheduler`], that
//! abstracts over the two supported backends:
//!
//! | Mode                          | Backend                | Use case               |
//! |-------------------------------|------------------------|------------------------|
//! | [`DeploymentMode::Standalone`] | [`StandaloneScheduler`] | Local / dev / single-node |
//! | [`DeploymentMode::Micro`]      | [`BollardScheduler`]    | Fixed-size container fleet |
//! | [`DeploymentMode::Elastic`]    | [`BollardScheduler`]    | Auto-scaling container fleet |
//!
//! # Cross-module context
//!
//! This module bridges the core `DeploymentMode` configuration with the
//! worker-specific scheduler implementations defined in sibling modules.
//!
//! Dependency: clawz_core::deployment::DeploymentMode
//! Dependency: clawz_core::traits::AgentScheduler

use std::sync::Arc;

// Dependency: core crate — shared configuration and trait definitions
use clawz_core::{deployment::DeploymentMode, error::Result, traits::AgentScheduler};

// Dependency: sibling modules — concrete scheduler implementations
use super::{BollardScheduler, StandaloneScheduler};
use super::tool_orchestrator::{DockerToolOrchestrator, InMemoryToolOrchestrator};
use clawz_core::traits::ToolOrchestrator;
use clawz_core::types::orchestration::AgentSpec;

/// Default agent image from environment (`CLAWZ_AGENT_IMAGE` or registry/tag).
pub fn env_agent_image() -> String {
    std::env::var("CLAWZ_AGENT_IMAGE").unwrap_or_else(|_| {
        let registry =
            std::env::var("CLAWZ_REGISTRY").unwrap_or_else(|_| "ghcr.io/improwyz".to_string());
        let tag = std::env::var("CLAWZ_IMAGE_TAG").unwrap_or_else(|_| "latest".to_string());
        format!("{registry}/clawz-agent:{tag}")
    })
}

/// Docker network for spawned agent/tool containers (compose: `clawz-net`).
pub fn env_docker_network() -> String {
    std::env::var("CLAWZ_DOCKER_NETWORK").unwrap_or_else(|_| "clawz-net".to_string())
}

/// Maximum concurrent agent containers on this worker host.
pub fn env_max_agents() -> usize {
    std::env::var("CLAWZ_MAX_AGENTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
}

/// [`AgentSpec`] with image and limits from environment.
pub fn default_agent_spec() -> AgentSpec {
    let mut spec = AgentSpec::default();
    spec.image = env_agent_image();
    spec
}

/// Creates an [`AgentScheduler`] appropriate for the given deployment mode.
///
/// The returned scheduler is wrapped in an [`Arc`] so it can be shared across
/// async tasks safely.
///
/// # Mode mapping
///
/// - [`DeploymentMode::Standalone`] → [`StandaloneScheduler`] with a fixed
///   worker pool of 4 threads.
/// - [`DeploymentMode::Micro`] / [`DeploymentMode::Elastic`] →
///   [`BollardScheduler`] connected to the local Docker daemon with a max
///   capacity of 10 containers.
///
/// # Errors
///
/// Returns an error if the selected backend fails to initialise (e.g. Docker
/// daemon is unreachable when `Micro` or `Elastic` is requested).
pub fn create_scheduler(mode: DeploymentMode) -> Result<Arc<dyn AgentScheduler>> {
    match mode {
        // Standalone mode: no Docker required; agents run as in-process tasks.
        DeploymentMode::Standalone => Ok(Arc::new(StandaloneScheduler::new(4))),
        // Containerised modes: use Docker via Bollard.
        // Both Micro and Elastic map to the same backend; elasticity is handled
        // at a higher level ( orchestrator / autoscaler ) rather than here.
        DeploymentMode::Micro | DeploymentMode::Elastic => {
            let scheduler =
                BollardScheduler::with_network(env_max_agents(), env_docker_network())?;
            Ok(Arc::new(scheduler))
        }
    }
}

/// Tool orchestrator for isolated tool types (browser, sandbox, MCP bridge).
pub fn create_tool_orchestrator(mode: DeploymentMode) -> Result<Arc<dyn ToolOrchestrator>> {
    match mode {
        DeploymentMode::Standalone => Ok(Arc::new(InMemoryToolOrchestrator::new())),
        DeploymentMode::Micro | DeploymentMode::Elastic => {
            let orch = DockerToolOrchestrator::with_network(env_docker_network())?;
            Ok(Arc::new(orch))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_standalone_returns_scheduler() {
        let scheduler = create_scheduler(DeploymentMode::Standalone);
        assert!(scheduler.is_ok());
    }

    #[test]
    fn factory_micro_attempts_bollard() {
        // This may fail without Docker daemon, that's OK — just verify code path
        let _result = create_scheduler(DeploymentMode::Micro);
    }
}
