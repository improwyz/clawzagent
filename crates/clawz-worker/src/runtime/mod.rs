//! Runtime module — orchestrates agent execution, pipeline processing, and team coordination.
//!
//! This module is the beating heart of the `clawz-worker` crate. It wires together:
//! - [`AgentRuntime`] – the main per-agent execution engine (single-turn & multi-turn).
//! - [`Pipeline`] – ordered, conditional step execution with automatic rollback.
//! - [`Team`] – leader + worker pool with least-busy load balancing.
//! - [`SubAgent`] & [`SubAgentPool`] – lightweight child-agents for parallel subtasks.
//! - [`FanOut`] & [`MixtureOfAgents`] – parallel provider invocation and response synthesis.
//! - [`Workflow`] – DAG-based step execution with saga-pattern compensations.
//!
//! # Key architectural invariant
//! Every request path ultimately funnels through a [`Pipeline`] composed of
//! [`PipelineStep`] implementations. This keeps concerns separated and makes
//! the system testable: each step can be unit-tested in isolation, and the
//! pipeline guarantees rollback semantics when any step fails.
//!
//! # Dependencies
//! - `clawz_core::traits` — [`PipelineStep`], [`PipelineContext`], [`GovernanceEngine`], [`MemoryBackend`]
//! - `clawz_core::types` — [`AgentConfig`], [`Message`], [`AgentState`]
//! - `crate::providers` — [`ProviderRouter`], [`CostTracker`]
//! - `steps` submodule — concrete [`PipelineStep`] implementations
//!
//! # Cross-module relationships
//! - `AgentRuntime` builds a [`Pipeline`] on every turn; the pipeline delegates
//!   to the `steps` submodule.
//! - `Team` delegates work by selecting the least-busy member and invoking its
//!   `AgentRuntime`.
//! - `FanOut` and `MixtureOfAgents` bypass the pipeline and talk directly to
//!   `ProviderRouter` for parallel inference.
//! - `Workflow` is a higher-level abstraction that can embed `AgentRuntime`
//!   calls inside its step handlers.

pub mod identity_types;

pub mod agent;
pub mod complexity;
pub mod fan_out;
pub mod identity;
pub mod idempotency;
pub mod negotiation;
pub mod orchestration;
pub mod pipeline;
pub mod spawner;
pub mod steps;
pub mod subagent;
pub mod team;
pub mod swarm;

/// Re-export the primary runtime entry point and its dependency bag.
pub use agent::{AgentRuntime, RuntimeDependencies};
/// Re-export the pipeline type for consumers that need to build custom pipelines.
pub use pipeline::Pipeline;
/// Re-export the spawner types for consumers that need scale decisions.
pub use spawner::{AgentTreeSpawner, ScaleDecision, ScalePolicy};
/// Re-export the complexity analyzer for consumers that need to size sub-agent teams.
pub use complexity::{ComplexityScore, TaskComplexity, TaskComplexityAnalyzer};