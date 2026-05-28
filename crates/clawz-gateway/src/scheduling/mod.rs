//! Scheduling module for the Clawz Gateway.
//!
//! This module is responsible for admission control and tenant-aware agent routing
//! at the HTTP entry point. It ensures that tenant concurrency limits are respected
//! and that incoming requests are dispatched to the most appropriate agent instance.
//!
//! # Key Components
//!
//! - [`AdmissionController`]: Enforces per-tenant concurrency quotas and issues
//!   admission tickets before requests proceed to agent scheduling.
//! - [`TenantRouter`]: Routes tenant requests to warm (already running) agents when
//!   possible, falling back to spawning new agent instances.
//!
//! # Architecture
//!
//! The scheduling layer sits between the HTTP handlers and the orchestrator:
//!
//! 1. HTTP handlers call [`AdmissionController::admit`] to acquire a ticket.
//! 2. On success, the [`TenantRouter`] selects or spawns an [`AgentHandle`].
//! 3. The ticket is later released via [`AdmissionController::release`].
//!
//! // Dependency: `clawz_core::traits::AgentScheduler` — trait implemented by the
//! // orchestrator and consumed by [`TenantRouter`] to perform actual agent lifecycle
//! // operations (spawn, reap, health checks).
//! // Dependency: `clawz_core::types::tenant::TenantContext` — shared tenant identity
//! // and role context used by both admission and routing decisions.

pub mod admission;
pub mod context;
pub mod tenant_router;

pub use admission::{AdmissionController, AdmissionTicket};
pub use context::tenant_context_from_auth;
pub use tenant_router::TenantRouter;
