//! Per-tenant admission control for agent scheduling.
//!
//! The [`AdmissionController`] guards the gateway against unbounded concurrent agent
//! creation by enforcing a per-tenant limit. Before the [`TenantRouter`] attempts
//! to find or spawn an agent, the gateway must first obtain an [`AdmissionTicket`].
//! This ticket acts as a concurrency lease: it reserves a slot and must be released
//! when the agent interaction completes (or fails), ensuring the slot is returned
//! to the pool for that tenant.
//!
//! # Design Rationale
//!
//! - Quotas are tracked in memory with a [`tokio::sync::RwLock<HashMap>`] because
//!   admission decisions must be fast and do not require persistent state. A restart
//!   resets counters, which is acceptable for soft-concurrency limits.
//! - `max_queue_depth` is currently accepted at construction but unused; it is
//!   reserved for future back-pressure mechanisms (e.g., queueing rather than
//!   immediately rejecting when the limit is hit).
//!
//! // Dependency: `clawz_core::types::tenant::TenantContext` — provides tenant identity
//! // used as the lookup key for per-tenant slot tracking.
//! // Dependency: `clawz_core::error::ClawzError` — admission failures surface as
//! // `RateLimited` errors so upstream HTTP layers can return 429 Too Many Requests.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use clawz_core::{
    error::{ClawzError, Result},
    types::tenant::TenantContext,
};

/// A ticket issued by the [`AdmissionController`] granting a single concurrent slot
/// to a tenant.
///
/// The ticket must be held for the lifetime of the agent interaction and passed
/// back to [`AdmissionController::release`] when the interaction ends. Dropping the
/// ticket without releasing it leaks the slot, so callers are responsible for
/// deterministic cleanup (e.g., `defer`-style patterns or `Drop` guards in higher
/// layers).
#[derive(Debug, Clone)]
pub struct AdmissionTicket {
    /// Unique identifier for this ticket, generated with `Uuid::new_v4`. Used to
    /// precisely remove the correct slot during release, avoiding accidental removal
    /// of another tenant's concurrent request.
    pub id: Uuid,
    /// The tenant that was granted this slot. Mirrors the tenant ID from the
    /// [`TenantContext`] supplied at admission time.
    pub tenant_id: String,
}

/// Enforces a soft concurrency limit per tenant before agent scheduling begins.
///
/// [`AdmissionController`] maintains an in-memory map of active tickets keyed by
/// tenant ID. Each successful [`admit`](Self::admit) call consumes one slot; each
/// [`release`](Self::release) call returns it. When a tenant has exhausted its
/// quota, subsequent admission attempts are rejected with a rate-limit error.
pub struct AdmissionController {
    /// Maximum number of concurrent agent interactions allowed for a single tenant.
    /// Once this threshold is reached, [`admit`](Self::admit) returns an error.
    max_concurrent_per_tenant: usize,
    /// Reserved for future queue-based back-pressure. Currently unused but kept in
    /// the struct to avoid breaking the public constructor signature when queuing
    /// is implemented.
    #[allow(dead_code)]
    max_queue_depth: usize,
    /// In-memory tracking of all active admission tickets, grouped by tenant ID.
    ///
    /// An [`Arc<RwLock<HashMap>>`] is used because the controller is shared across
    /// many concurrent HTTP requests: readers check current counts without blocking
    /// each other, while writers (admit/release) acquire exclusive access only for
    /// the brief duration of map mutation.
    active: Arc<RwLock<HashMap<String, Vec<Uuid>>>>,
}

impl AdmissionController {
    /// Creates a new admission controller with the given limits.
    ///
    /// # Arguments
    ///
    /// * `max_concurrent_per_tenant` — Hard cap on simultaneous agent sessions per
    ///   tenant. Must be > 0 for the controller to admit any requests.
    /// * `max_queue_depth` — Soft reservation for future queueing support. Currently
    ///   has no effect on admission behavior.
    pub fn new(max_concurrent_per_tenant: usize, max_queue_depth: usize) -> Self {
        Self {
            max_concurrent_per_tenant,
            max_queue_depth,
            active: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Attempts to admit a new request for the tenant described by `ctx`.
    ///
    /// If the tenant has not yet reached [`max_concurrent_per_tenant`](Self::max_concurrent_per_tenant),
    /// a new [`AdmissionTicket`] is issued and the tenant's active slot count is
    /// incremented. Otherwise, a [`ClawzError::RateLimited`] is returned with a fixed
    /// 5-second `retry_after_secs` hint.
    ///
    /// # Errors
    ///
    /// Returns `ClawzError::RateLimited` when the per-tenant concurrent limit is
    /// exceeded.
    pub async fn admit(&self, ctx: &TenantContext) -> Result<AdmissionTicket> {
        // Acquire write lock because we may need to insert a new tenant entry or
        // push a new ticket ID into the active list. The lock is held only for
        // this brief critical section to keep contention minimal under load.
        let mut active = self.active.write().await;

        // Clone the tenant key so we can use it for both HashMap lookup and the
        // ticket itself without borrowing from ctx after the entry API consumes it.
        let tenant_key = ctx.tenant_id.as_str().to_string();

        // or_default() lazily creates an empty Vec for new tenants, keeping the
        // map sparse so tenants with no active requests consume no memory.
        let slots = active.entry(tenant_key.clone()).or_default();

        // Reject immediately rather than queueing. This prevents the gateway from
        // becoming a bottleneck and pushes back-pressure to the caller (HTTP 429).
        if slots.len() >= self.max_concurrent_per_tenant {
            return Err(ClawzError::RateLimited { retry_after_secs: 5 });
        }

        // Generate a fresh UUID so release() can pinpoint this exact slot even if
        // the same tenant has many overlapping admissions.
        let ticket = AdmissionTicket {
            id: Uuid::new_v4(),
            tenant_id: tenant_key,
        };
        slots.push(ticket.id);
        Ok(ticket)
    }

    /// Releases a previously issued [`AdmissionTicket`], returning its slot to the
    /// tenant's available pool.
    ///
    /// If the ticket is not found (e.g., the controller was restarted or the ticket
    /// was double-released), this method silently succeeds. This idempotency prevents
    /// a failed release from cascading into a hard error in cleanup paths.
    pub async fn release(&self, ticket: AdmissionTicket) {
        let mut active = self.active.write().await;
        if let Some(slots) = active.get_mut(&ticket.tenant_id) {
            // retain() is used instead of a single remove() because Vec does not
            // guarantee O(1) removal by index without shifting; however, since the
            // number of concurrent slots per tenant is small (typically < 100), the
            // linear scan is cheap and avoids needing a more complex data structure.
            slots.retain(|id| *id != ticket.id);

            // Optional cleanup: if the tenant has no remaining slots, we could remove
            // the entry here. It is intentionally omitted to avoid additional write
            // churn; empty vectors consume negligible memory.
        }
    }

    /// Returns the number of currently active (not yet released) tickets for the
    /// given tenant.
    ///
    /// This is primarily useful for metrics, observability, and tests. It acquires
    /// a read lock, so it will not block concurrent admit/release operations.
    pub async fn active_count(&self, tenant_id: &str) -> usize {
        self.active
            .read()
            .await
            .get(tenant_id)
            .map(|s| s.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::tenant::{Role, TenantId};

    #[tokio::test]
    async fn admits_under_limit() {
        let ctrl = AdmissionController::new(10, 100);
        let ctx = TenantContext::new(TenantId::new("t1"), Role::Owner);
        assert!(ctrl.admit(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_over_concurrent_limit() {
        let ctrl = AdmissionController::new(1, 100);
        let ctx = TenantContext::new(TenantId::new("t1"), Role::Owner);
        ctrl.admit(&ctx).await.unwrap();
        let result = ctrl.admit(&ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn release_frees_slot() {
        let ctrl = AdmissionController::new(1, 100);
        let ctx = TenantContext::new(TenantId::new("t1"), Role::Owner);
        let ticket = ctrl.admit(&ctx).await.unwrap();
        ctrl.release(ticket).await;
        assert!(ctrl.admit(&ctx).await.is_ok());
    }
}
