//! Tenant-aware agent routing for the Clawz Gateway.
//!
//! The [`TenantRouter`] is the bridge between admission-controlled HTTP requests
//! and the orchestrator's agent pool. Its primary responsibility is to decide
//! whether an incoming tenant request can be served by an existing "warm" agent
//! (one that is already running and ready) or whether a new agent must be spawned.
//!
//! # Routing Strategy
//!
//! 1. **Warm preference** — First, query the underlying [`AgentScheduler`] for a
//!    warm agent matching the requested capabilities and tenant context. Reusing
//!    warm agents avoids cold-start latency and redundant resource usage.
//! 2. **Spawn fallback** — If no warm agent is available, the router delegates to
//!    the scheduler to spawn a new agent with default specifications.
//!
//! This module intentionally keeps routing logic thin; complex scheduling policies
//! (e.g., load balancing, affinity, or pre-warming) belong in the orchestrator's
//! [`AgentScheduler`] implementation rather than here.
//!
//! // Dependency: `clawz_core::traits::AgentScheduler` — abstract scheduler trait
//! // implemented by the orchestrator. The router is agnostic to the concrete
//! // scheduler backend (e.g., in-process, Kubernetes, or remote pool).
//! // Dependency: `clawz_core::types::orchestration::{AgentHandle, AgentSpec}` —
//! // descriptors for agent instances and their creation parameters.
//! // Dependency: `clawz_core::types::tenant::TenantContext` — tenant identity and
//! // role context forwarded to the scheduler for tenant-scoped lookups.

use std::sync::Arc;

use clawz_core::{
    error::Result,
    traits::AgentScheduler,
    types::orchestration::{AgentHandle, AgentSpec},
    types::tenant::TenantContext,
};

/// Routes tenant requests to agents via the underlying scheduler.
///
/// [`TenantRouter`] does not own agent processes directly; instead, it wraps an
/// [`Arc<dyn AgentScheduler>`] and applies a simple warm-then-spawn policy. It is
/// designed to be cheaply cloneable (via the `Arc`) so that it can be shared across
/// all HTTP handler tasks in the gateway.
pub struct TenantRouter {
    /// The concrete scheduler backend that performs actual agent lifecycle operations.
    ///
    /// Using `Arc<dyn AgentScheduler>` allows the gateway to share a single scheduler
    /// instance across many concurrent requests without taking a dependency on the
    /// orchestrator's concrete type, keeping compile times and coupling low.
    scheduler: Arc<dyn AgentScheduler>,
}

impl TenantRouter {
    /// Creates a new router backed by the provided scheduler.
    ///
    /// # Arguments
    ///
    /// * `scheduler` — An [`Arc`] to an implementation of [`AgentScheduler`].
    ///   Typically this is constructed in the gateway's startup sequence and wired
    ///   into the Axum / HTTP router state.
    pub fn new(scheduler: Arc<dyn AgentScheduler>) -> Self {
        Self { scheduler }
    }

    /// Routes a tenant request to the best available agent.
    ///
    /// The router first attempts to find a warm agent that matches the tenant and
    /// requested capabilities. This minimizes latency because the agent is already
    /// initialized and ready to accept tasks. If no warm agent exists, it falls
    /// back to spawning a new agent with [`AgentSpec::default`].
    ///
    /// # Arguments
    ///
    /// * `ctx` — The tenant context carrying identity and role information. Forwarded
    ///   to the scheduler so that agent lookups are scoped to the correct tenant.
    /// * `capabilities` — A list of capability strings (e.g., `["llm", "code-search"]`)
    ///   that the target agent must satisfy. The scheduler uses these to filter warm
    ///   candidates.
    ///
    /// # Errors
    ///
    /// Returns errors from the scheduler's `find_warm` or `spawn_agent` operations,
    /// such as resource exhaustion, scheduler communication failures, or tenant
    /// isolation violations.
    pub async fn route(&self, ctx: &TenantContext, capabilities: &[String]) -> Result<AgentHandle> {
        // Prefer warm agents because spawning incurs cold-start latency (loading
        // models, establishing tool connections, etc.) and consumes fresh resources.
        if let Some(handle) = self.scheduler.find_warm(ctx, capabilities).await {
            return Ok(handle);
        }

        // No warm match found; ask the scheduler to create a new agent. We use
        // the default AgentSpec here because the gateway currently does not pass
        // per-request resource sizing (memory, CPU, model variants) down from HTTP
        // headers or body. If that becomes a requirement, the spec should be
        // constructed from ctx or an external profile store before this call.
        self.scheduler.spawn_agent(ctx, AgentSpec::default()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::orchestration::HealthStatus;

    #[test]
    fn tenant_router_module_loads() {
        let _router = TenantRouter::new(Arc::new(NullScheduler));
        assert!(true);
    }

    struct NullScheduler;

    #[async_trait::async_trait]
    impl AgentScheduler for NullScheduler {
        async fn spawn_agent(
            &self,
            ctx: &TenantContext,
            _spec: AgentSpec,
        ) -> Result<AgentHandle> {
            Ok(AgentHandle::new(
                ctx.tenant_id.clone(),
                "agent".into(),
                "127.0.0.1".into(),
            ))
        }

        async fn reap_agent(&self, _handle: &AgentHandle) -> Result<()> {
            Ok(())
        }

        async fn find_warm(
            &self,
            _ctx: &TenantContext,
            _capabilities: &[String],
        ) -> Option<AgentHandle> {
            None
        }

        async fn list_agents(&self, _tenant_id: &str) -> Result<Vec<AgentHandle>> {
            Ok(vec![])
        }

        async fn health(&self, _handle: &AgentHandle) -> Result<HealthStatus> {
            Ok(HealthStatus {
                readiness: 1.0,
                liveness: true,
                tool_slots_available: 1,
                memory_usage_percent: 0.0,
                last_check: chrono::Utc::now(),
            })
        }
    }
}
