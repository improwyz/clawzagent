//! Approval workflows — request, approve, reject, escalate, and expire.
//!
//! This module implements an in-memory approval workflow engine for governance
//! actions. Each [`ApprovalWorkflow`] maintains a registry of [`ApprovalRequest`]
//! instances and tracks their lifecycle through the [`ApprovalStatus`] states:
//!
//! # ApprovalStatus lifecycle
//!
//! ```text
//! Pending ──► Approved
//!    │
//!    ├──────► Rejected
//!    │
//!    └──────► Expired   (TTL elapsed or explicit expiry check)
//! ```
//!
//! * **Pending** — The request is active and awaiting the required number of
//!   distinct approver signatures (`required_approvals`).
//! * **Approved** — The request has received enough approvals and is considered
//!   fully authorised. This is a terminal state.
//! * **Rejected** — An approver explicitly denied the request. This is a
//!   terminal state; no further transitions are allowed.
//! * **Expired** — The request’s `expires_at` timestamp has passed. This is
//!   also a terminal state.
//!
//! # Escalation chains
//!
//! Escalation chains are **per-action** ordered lists of [`EscalationLevel`]
//! entries. Each level defines a set of authorised `approver_ids` and a
//! `timeout`.  If the required approvals are not gathered before the level’s
//! timeout elapses, the workflow may advance to the next level (caller-driven;
//! this module stores the chain definition but does not automatically advance
//! timers yet).
//!
//! Register a chain with [`ApprovalWorkflow::register_escalation`]:
//!
//! ```rust,ignore
//! workflow.register_escalation(
//!     "deploy",
//!     vec![
//!         EscalationLevel {
//!             approver_ids: vec!["team-lead".into()],
//!             timeout: Duration::from_secs(300),
//!         },
//!         EscalationLevel {
//!             approver_ids: vec!["director".into()],
//!             timeout: Duration::from_secs(600),
//!         },
//!     ],
//! );
//! ```
//!
//! # Expiry TTL
//!
//! Every request carries a `expires_at` field.  The default TTL is
//! [`DEFAULT_EXPIRY_SECS`] (1 hour) and can be customised via
//! [`ApprovalWorkflow::with_default_expiry`].  Expiry is evaluated lazily on
//! status checks ([`check_status`], [`approve`], [`list_pending`]) and eagerly
//! by [`expire_stale`].
//!
//! # Persistence dependencies
//!
//! > **In-memory only.**  The current implementation stores all requests in an
//! > `Arc<RwLock<HashMap<String, ApprovalRequest>>>`.  Production deployments
//! > **must** replace or wrap this storage with a persistent backend (e.g.
//! > PostgreSQL, Redis, or a write-ahead log) so that approvals survive process
//! > restarts and can be queried across replicas.  All public methods are
//! > `async`, making it straightforward to swap the `RwLock<HashMap>` for an
//! > async-backed store without changing call-sites.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clawz_core::{
    error::{ClawzError, Result},
    types::governance::{ApprovalRequest, ApprovalStatus},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Default TTL for approval requests (1 hour).
const DEFAULT_EXPIRY_SECS: i64 = 3600;

// ── EscalationLevel ────────────────────────────────────────────────────────────

/// One level in a per-action escalation chain.
///
/// An escalation chain is an ordered vector of [`EscalationLevel`] values.
/// Each level lists the identities that may approve at that tier and the
/// maximum duration the request may remain at this level before the caller
/// should consider advancing to the next tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscalationLevel {
    /// Identities authorised to approve at this level.
    pub approver_ids: Vec<String>,
    /// Maximum wall-clock time the request may sit at this level.
    pub timeout: Duration,
}

// ── ApprovalWorkflow ──────────────────────────────────────────────────────────

/// In-memory approval workflow engine.
///
/// Holds the authoritative registry of [`ApprovalRequest`] instances, per-action
/// escalation chains, and the default TTL for newly created requests.
///
/// # Concurrency
///
/// The request store is wrapped in [`tokio::sync::RwLock`] behind an [`Arc`],
/// so a single [`ApprovalWorkflow`] instance may be cloned cheaply and shared
/// across tasks.
///
/// # Persistence note
///
/// See the [module-level documentation](crate::governance::approval) for
/// persistence requirements.
pub struct ApprovalWorkflow {
    /// Active and terminal approval requests keyed by request ID.
    ///
    /// **Persistence dependency:** this `HashMap` is in-memory only.
    requests: Arc<RwLock<HashMap<String, ApprovalRequest>>>,
    /// Per-action escalation chains (`action` → ordered levels).
    escalation_chains: HashMap<String, Vec<EscalationLevel>>,
    /// Fallback TTL applied to new requests when the caller does not supply one.
    default_expiry: Duration,
}

impl ApprovalWorkflow {
    /// Creates a new workflow with the default 1-hour expiry and no escalation
    /// chains registered.
    pub fn new() -> Self {
        Self {
            requests: Arc::new(RwLock::new(HashMap::new())),
            escalation_chains: HashMap::new(),
            default_expiry: Duration::from_secs(DEFAULT_EXPIRY_SECS as u64),
        }
    }

    /// Overrides the default TTL applied to every new request.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let wf = ApprovalWorkflow::new()
    ///     .with_default_expiry(Duration::from_secs(600));
    /// ```
    pub fn with_default_expiry(mut self, expiry: Duration) -> Self {
        self.default_expiry = expiry;
        self
    }

    /// Registers an escalation chain for a specific action.
    ///
    /// When an action is requested, the caller may look up the chain via
    /// `action` and drive multi-tier approval logic.  This module stores the
    /// definition but does **not** automatically advance timers between levels;
    /// that responsibility belongs to the orchestrator layer.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::time::Duration;
    ///
    /// let wf = ApprovalWorkflow::new()
    ///     .register_escalation(
    ///         "delete",
    ///         vec![
    ///             EscalationLevel {
    ///                 approver_ids: vec!["ops".into()],
    ///                 timeout: Duration::from_secs(300),
    ///             },
    ///             EscalationLevel {
    ///                 approver_ids: vec!["sre".into()],
    ///                 timeout: Duration::from_secs(600),
    ///             },
    ///         ],
    ///     );
    /// ```
    pub fn register_escalation(
        mut self,
        action: impl Into<String>,
        levels: Vec<EscalationLevel>,
    ) -> Self {
        self.escalation_chains.insert(action.into(), levels);
        self
    }

    // ── Request / Approve / Reject ─────────────────────────────────────────────

    /// Creates a new [`ApprovalRequest`] in [`ApprovalStatus::Pending`] state.
    ///
    /// The request is stamped with `expires_at = now + default_expiry` and
    /// inserted into the in-memory store.
    ///
    /// # Arguments
    ///
    /// * `agent_id` — Identity originating the action.
    /// * `action`   — Arbitrary action tag (e.g. `"deploy"`, `"delete"`).
    /// * `context`  — Free-form JSON payload describing the request.
    /// * `required_approvals` — Number of distinct approver signatures needed
    ///   before the request transitions to [`ApprovalStatus::Approved`].
    ///
    /// # Returns
    ///
    /// The unique approval request ID (`Uuid` as a string) that callers must
    /// retain for subsequent `approve`, `reject`, or `check_status` calls.
    pub async fn request(
        &self,
        agent_id: impl Into<String>,
        action: impl Into<String>,
        context: serde_json::Value,
        required_approvals: u32,
    ) -> String {
        let expires_at =
            Utc::now() + chrono::Duration::from_std(self.default_expiry).unwrap_or_default();
        let req = ApprovalRequest::new(agent_id, action, context, required_approvals)
            .with_expiry(expires_at);
        let id = req.id.clone();
        self.requests.write().await.insert(id.clone(), req);
        log::debug!("[approval] created request id={}", id);
        id
    }

    /// Records an approval for the given request.
    ///
    /// If the request has already passed its `expires_at`, it is moved to
    /// [`ApprovalStatus::Expired`] and an error is returned.
    ///
    /// # Arguments
    ///
    /// * `approval_id`  — The ID returned by [`request`].
    /// * `approver_id`  — Identity of the human or system signing off.
    ///
    /// # Returns
    ///
    /// * `Ok(true)`  — The request has reached `required_approvals` and is now
    ///   [`ApprovalStatus::Approved`].
    /// * `Ok(false)` — The approval was recorded but additional signatures are
    ///   still required.
    /// * `Err(ClawzError::NotFound)` — Unknown `approval_id`.
    /// * `Err(ClawzError::Validation)` — The request has expired.
    pub async fn approve(
        &self,
        approval_id: &str,
        approver_id: &str,
    ) -> Result<bool> {
        let mut requests = self.requests.write().await;
        let req = requests.get_mut(approval_id).ok_or_else(|| {
            ClawzError::NotFound {
                entity: "approval request".into(),
                id: approval_id.into(),
            }
        })?;

        // Check expiry.
        if req.is_expired() {
            req.status = ApprovalStatus::Expired;
            return Err(ClawzError::Validation(format!(
                "approval request '{}' has expired",
                approval_id
            )));
        }

        let fully_approved = req.approve(approver_id);
        log::info!(
            "[approval] id={} approver='{}' fully_approved={}",
            approval_id,
            approver_id,
            fully_approved
        );
        Ok(fully_approved)
    }

    /// Rejects the request, immediately moving it to [`ApprovalStatus::Rejected`].
    ///
    /// # Arguments
    ///
    /// * `approval_id` — The ID returned by [`request`].
    /// * `approver_id` — Identity performing the rejection.
    /// * `reason`      — Human-readable rationale logged for audit purposes.
    ///
    /// # Returns
    ///
    /// * `Ok(())` on success.
    /// * `Err(ClawzError::NotFound)` if the ID does not exist.
    pub async fn reject(&self, approval_id: &str, approver_id: &str, reason: &str) -> Result<()> {
        let mut requests = self.requests.write().await;
        let req = requests.get_mut(approval_id).ok_or_else(|| {
            ClawzError::NotFound {
                entity: "approval request".into(),
                id: approval_id.into(),
            }
        })?;
        req.reject();
        log::info!(
            "[approval] id={} rejected by '{}': {}",
            approval_id,
            approver_id,
            reason
        );
        Ok(())
    }

    // ── Status / Query ────────────────────────────────────────────────────────

    /// Returns the current [`ApprovalStatus`] for a request.
    ///
    /// Lazily evaluates expiry: if the request is [`ApprovalStatus::Pending`] and
    /// `is_expired()` is true, the status is promoted to
    /// [`ApprovalStatus::Expired`] before being returned.
    ///
    /// # Returns
    ///
    /// * `Ok(ApprovalStatus)` — The current (possibly newly expired) status.
    /// * `Err(ClawzError::NotFound)` if the ID does not exist.
    pub async fn check_status(&self, approval_id: &str) -> Result<ApprovalStatus> {
        let mut requests = self.requests.write().await;
        let req = requests.get_mut(approval_id).ok_or_else(|| {
            ClawzError::NotFound {
                entity: "approval request".into(),
                id: approval_id.into(),
            }
        })?;
        // Auto-expire if needed.
        if req.status == ApprovalStatus::Pending && req.is_expired() {
            req.status = ApprovalStatus::Expired;
        }
        Ok(req.status.clone())
    }

    /// Retrieves a full clone of the [`ApprovalRequest`] by ID.
    ///
    /// Returns `None` if the ID is not present.  No expiry side-effects are
    /// performed; use [`check_status`] if you need auto-expiry evaluation.
    pub async fn get_request(&self, approval_id: &str) -> Option<ApprovalRequest> {
        self.requests
            .read()
            .await
            .get(approval_id)
            .cloned()
    }

    /// Lists all requests that are still [`ApprovalStatus::Pending`] and have
    /// not yet reached their `expires_at` timestamp.
    ///
    /// Expired entries are filtered out but **not** promoted to
    /// [`ApprovalStatus::Expired`] by this call.
    pub async fn list_pending(&self) -> Vec<ApprovalRequest> {
        let requests = self.requests.read().await;
        requests
            .values()
            .filter(|r| r.status == ApprovalStatus::Pending && !r.is_expired())
            .cloned()
            .collect()
    }

    /// Lists every request (all statuses) originated by the given `agent_id`.
    pub async fn list_by_agent(&self, agent_id: &str) -> Vec<ApprovalRequest> {
        self.requests
            .read()
            .await
            .values()
            .filter(|r| r.agent_id == agent_id)
            .cloned()
            .collect()
    }

    /// Scans all [`ApprovalStatus::Pending`] requests and promotes any whose
    /// `expires_at` has passed to [`ApprovalStatus::Expired`].
    ///
    /// Call this from a background janitor task (e.g. every 60 s) to keep the
    /// store free of stale entries.
    ///
    /// # Returns
    ///
    /// The number of requests transitioned to [`ApprovalStatus::Expired`].
    pub async fn expire_stale(&self) -> usize {
        let mut requests = self.requests.write().await;
        let mut count = 0;
        for req in requests.values_mut() {
            if req.status == ApprovalStatus::Pending && req.is_expired() {
                req.status = ApprovalStatus::Expired;
                count += 1;
            }
        }
        if count > 0 {
            log::info!("[approval] expired {} stale requests", count);
        }
        count
    }

    /// Returns the total number of requests currently held in memory
    /// (including terminal states).
    pub async fn request_count(&self) -> usize {
        self.requests.read().await.len()
    }
}

impl Default for ApprovalWorkflow {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_request_creates_pending_entry() {
        let wf = ApprovalWorkflow::new();
        let id = wf
            .request("agent-1", "delete", serde_json::json!({}), 1)
            .await;
        let status = wf.check_status(&id).await.unwrap();
        assert_eq!(status, ApprovalStatus::Pending);
    }

    #[tokio::test]
    async fn test_single_approval_fully_approves() {
        let wf = ApprovalWorkflow::new();
        let id = wf
            .request("agent-1", "deploy", serde_json::json!({}), 1)
            .await;
        let fully = wf.approve(&id, "admin").await.unwrap();
        assert!(fully);
        let status = wf.check_status(&id).await.unwrap();
        assert_eq!(status, ApprovalStatus::Approved);
    }

    #[tokio::test]
    async fn test_two_required_not_full_after_one() {
        let wf = ApprovalWorkflow::new();
        let id = wf
            .request("agent-1", "deploy", serde_json::json!({}), 2)
            .await;
        let fully = wf.approve(&id, "admin1").await.unwrap();
        assert!(!fully);
        let status = wf.check_status(&id).await.unwrap();
        assert_eq!(status, ApprovalStatus::Pending);
    }

    #[tokio::test]
    async fn test_reject_changes_status() {
        let wf = ApprovalWorkflow::new();
        let id = wf
            .request("agent-1", "action", serde_json::json!({}), 1)
            .await;
        wf.reject(&id, "reviewer", "not appropriate").await.unwrap();
        let status = wf.check_status(&id).await.unwrap();
        assert_eq!(status, ApprovalStatus::Rejected);
    }

    #[tokio::test]
    async fn test_expiry() {
        let wf = ApprovalWorkflow::new().with_default_expiry(Duration::from_millis(1));
        let id = wf
            .request("agent-1", "action", serde_json::json!({}), 1)
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        let status = wf.check_status(&id).await.unwrap();
        assert_eq!(status, ApprovalStatus::Expired);
    }

    #[tokio::test]
    async fn test_list_pending_filters_expired_and_approved() {
        let wf = ApprovalWorkflow::new();
        let id1 = wf.request("a1", "act1", serde_json::json!({}), 1).await;
        let id2 = wf.request("a2", "act2", serde_json::json!({}), 1).await;
        wf.approve(&id2, "admin").await.unwrap();

        let pending = wf.list_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id1);
    }

    #[tokio::test]
    async fn test_not_found_returns_error() {
        let wf = ApprovalWorkflow::new();
        let result = wf.check_status("nonexistent").await;
        assert!(result.is_err());
    }
}
