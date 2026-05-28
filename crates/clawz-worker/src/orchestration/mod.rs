//! Orchestration module for agent scheduling and lifecycle management.
//!
//! This module is the heart of the worker crate's execution layer. It defines
//! the strategies used to spawn, track, and reap agent containers (or
//! in-process tasks) according to the configured [`DeploymentMode`].
//!
//! # Sub-modules
//!
//! - [`factory`]: Entry-point for creating the correct scheduler backend.
//! - [`bollard_scheduler`]: Docker-backed scheduler that runs each agent in
//!   an isolated container via the Bollard client.
//! - [`standalone`]: Lightweight in-process scheduler for single-node
//!   deployments.
//! - [`lifecycle`]: Tracks tool handles and performs idle / orphan clean-up.
//!
//! # Architecture context
//!
//! Workers execute via the `runtime` module. The 3 crates form a 3-tier
//! architecture: gateway (API layer) → worker (execution layer) → core
//! (shared types / traits).

pub mod bollard_scheduler;
pub mod factory;
pub mod lifecycle;
pub mod standalone;
pub mod tool_orchestrator;

// Re-export the primary public types so consumers can write:
// `use clawz_worker::orchestration::BollardScheduler;`

/// Docker-based scheduler backed by the Bollard Docker API.
pub use bollard_scheduler::BollardScheduler;

/// Factory function that selects a scheduler implementation based on the
/// current [`DeploymentMode`].
pub use factory::{
    create_scheduler, create_tool_orchestrator, default_agent_spec, env_agent_image,
    env_docker_network, env_max_agents,
};
pub use tool_orchestrator::{DockerToolOrchestrator, InMemoryToolOrchestrator};

/// Manager that tracks tool lifetimes and reaps idle or orphaned tools.
pub use lifecycle::LifecycleManager;

/// In-process scheduler for standalone (no-container) deployments.
pub use standalone::StandaloneScheduler;
