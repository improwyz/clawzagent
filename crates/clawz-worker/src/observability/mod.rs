//! Observability module for the worker crate.
//!
//! This module provides trace context propagation utilities that inject
//! OpenTelemetry and Clawz-specific environment variables into containers
//! and tool executions. It sits in the middle tier of the 3-tier architecture:
//! gateway (API layer) → **worker (execution layer)** → core (shared types/traits).
//!
//! The worker runtime uses these helpers to ensure every execution context
//! carries enough metadata for downstream observability pipelines to correlate
//! logs, traces, and metrics back to the originating tenant, agent, and tool.
//!
//! # Sub-modules
//! - `context_propagation` — Generates key-value environment variable vectors
//!   for container and tool processes.

// Dependency: re-exports consumed by the runtime module when spawning containers
// and by the tool executor when invoking external tool processes.
pub mod context_propagation;

pub use context_propagation::{trace_env_for_container, trace_env_for_tool};
