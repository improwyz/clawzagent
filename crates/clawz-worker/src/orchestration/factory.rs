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
            let scheduler = BollardScheduler::new(10)?;
            Ok(Arc::new(scheduler))
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
