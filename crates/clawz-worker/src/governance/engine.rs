//! Governance engine orchestration for the Clawz worker.
//!
//! This module provides [`ClawzGovernanceEngine`], the concrete implementation of the
//! [`GovernanceEngine`](clawz_core::traits::GovernanceEngine) trait defined in `clawz-core`.
//! It evaluates whether an agent is permitted to perform an action by composing
//! multiple subsystems into a single decision pipeline.
//!
//! # Subsystems
//!
//! | Subsystem            | Crate module                           | Responsibility                     |
//! |----------------------|----------------------------------------|------------------------------------|
//! | Policy engine        | [`crate::governance::policy`]           | Rule-based allow / deny / review   |
//! | Trust scorer         | [`crate::governance::trust`]            | Dynamic per-agent reputation       |
//! | Guardrails           | [`crate::governance::guardrails`]       | G-dimension safety/compliance checks |
//! | Approval workflow    | [`crate::governance::approval`]        | Human-in-the-loop escalation       |
//! | Audit logger         | [`crate::governance::audit`]            | Immutable evaluation history       |
//!
//! # Orchestration flow
//!
//! When [`evaluate`](GovernanceEngine::evaluate) is called, the engine executes the
//! following pipeline:
//!
//! 1. **Cache lookup** – short-TTL memoisation keyed by `(agent_id, action, context)`.
//!    Identical evaluations within the TTL window are returned verbatim.
//! 2. **Hard deny** – if the agent's trust score is at or below `deny_below_trust`,
//!    the action is immediately rejected without further checks.
//! 3. **Policy evaluation** – the [`PolicyEngine`] evaluates every registered
//!    [`GovernancePolicy`](clawz_core::types::governance::GovernancePolicy) against
//!    the action and context.
//!    * `Deny` effects accumulate into `violations`.
//!    * `Review` effects set `requires_review = true`.
//! 4. **Guardrail checks** — [`GovernanceGuardrails`] audits the action string against recent
//!    audit history across all guardrail checks. Failed checks are added as
//!    violations.
//! 5. **Trust review gate** – if the trust score is below `min_trust_score` (but
//!    above the hard deny threshold), the action is escalated to `pending_review`.
//! 6. **Result construction** –
//!    * Any violation → [`GovernanceResult::deny`].
//!    * Review required → [`GovernanceResult::pending_review`].
//!    * Otherwise → [`GovernanceResult::allow`].
//! 7. **Audit & cache** – the outcome is logged via [`AuditLogger`] and cached
//!    before returning.

// Dependency: `std::collections::HashMap` — in-memory cache backing store.
use std::collections::HashMap;
// Dependency: `std::sync::Arc` — shared ownership of subsystems across async tasks.
use std::sync::Arc;

// Dependency: `async_trait` — enables async methods in traits on older Rust editions.
use async_trait::async_trait;
// Dependency: `clawz_core` — defines the `GovernanceEngine` trait and shared types.
use clawz_core::{
    error::{ClawzError, Result},
    traits::GovernanceEngine,
    types::governance::{ApprovalRequest, GovernancePolicy, GovernanceResult},
};
// Dependency: `serde` — serialisation for configuration and cache entries.
use serde::{Deserialize, Serialize};
// Dependency: `tokio::sync::RwLock` — concurrent read-heavy access to mutable engines.
use tokio::sync::RwLock;

// Dependency: sibling governance subsystems in `crate::governance`.
use super::{
    approval::ApprovalWorkflow,
    audit::{AuditLogger, AuditResult},
    guardrails::GovernanceGuardrails,
    policy::PolicyEngine,
    trust::TrustScorer,
};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Tunable parameters controlling the behaviour of [`ClawzGovernanceEngine`].
///
/// All fields are public so that callers may construct the configuration via
/// struct literal or load it from a serialised source (JSON, YAML, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceEngineConfig {
    /// When `true`, the [`PolicyEngine`] is consulted during [`evaluate`](GovernanceEngine::evaluate).
    pub enable_policy_check: bool,
    /// When `true`, [`GovernanceGuardrails`] checks are evaluated.
    pub enable_guardrails: bool,
    /// When `true`, the agent's trust score gates the action.
    pub enable_trust_check: bool,
    /// Minimum trust score (0–1000) required to proceed *without* human approval.
    ///
    /// Scores below this trigger `pending_review` unless they hit the hard deny.
    pub min_trust_score: u16,
    /// Trust score floor (0–1000). Anything at or below this is an automatic deny.
    pub deny_below_trust: u16,
    /// When `true`, every evaluation result is appended to the [`AuditLogger`].
    pub audit_enabled: bool,
}

impl Default for GovernanceEngineConfig {
    /// Returns a conservative default configuration:
    ///
    /// * All checks enabled.
    /// * `min_trust_score` = 300.
    /// * `deny_below_trust` = 100.
    /// * `audit_enabled` = true.
    fn default() -> Self {
        Self {
            enable_policy_check: true,
            enable_guardrails: true,
            enable_trust_check: true,
            min_trust_score: 300,
            deny_below_trust: 100,
            audit_enabled: true,
        }
    }
}

// ── Result cache entry ────────────────────────────────────────────────────────

/// Internal cache entry pairing a [`GovernanceResult`] with an expiration time.
///
/// Used by [`ClawzGovernanceEngine`] to avoid recomputing identical evaluations
/// within a short TTL window. The cache is not persisted across restarts.
#[derive(Clone)]
struct CacheEntry {
    /// The cached evaluation outcome.
    result: GovernanceResult,
    /// Wall-clock instant after which this entry is considered stale.
    expires_at: std::time::Instant,
}

// ── ClawzGovernanceEngine ─────────────────────────────────────────────────────

/// Concrete implementation of the `clawz-core` [`GovernanceEngine`] trait.
///
/// Composes five sibling subsystems into a single evaluation pipeline:
///
/// | Field                | Responsibility                          |
/// |----------------------|-----------------------------------------|
/// | `policy_engine`      | Rule-based allow / deny / review        |
/// | `trust_scorer`       | Per-agent reputation (0.0 – 1.0)       |
/// | `guardrails`         | G-dimension compliance checks          |
/// | `approval_workflow`  | Human-in-the-loop escalation            |
/// | `audit_logger`       | Immutable evaluation history            |
///
/// The engine also maintains an in-memory cache with a configurable TTL
/// (default 5 s) to amortise the cost of repeated identical evaluations.
pub struct ClawzGovernanceEngine {
    /// Active configuration. Supplied at construction via [`new`](Self::new).
    config: GovernanceEngineConfig,
    // Dependency: `crate::governance::policy::PolicyEngine`
    /// Rule store and evaluator. Guarded by an async read–write lock.
    policy_engine: Arc<RwLock<PolicyEngine>>,
    // Dependency: `crate::governance::trust::TrustScorer`
    /// Reputation tracker for every known agent. Trust scores are stored on a
    /// 0–1000 integer scale and normalised to `f64` on read.
    trust_scorer: Arc<TrustScorer>,
    // Dependency: `crate::governance::guardrails::GovernanceGuardrails`
    /// Governance guardrails checker. Guarded by an async read–write lock.
    guardrails: Arc<RwLock<GovernanceGuardrails>>,
    // Dependency: `crate::governance::approval::ApprovalWorkflow`
    /// Human-in-the-loop approval queue.
    pub(crate) approval_workflow: Arc<ApprovalWorkflow>,
    // Dependency: `crate::governance::audit::AuditLogger`
    /// Immutable audit log. Every evaluation (and certain other operations)
    /// appends an entry when `audit_enabled` is true.
    audit_logger: Arc<AuditLogger>,
    // Dependency: `std::collections::HashMap`
    /// Short-TTL cache for identical evaluations keyed by
    /// `agent_id|action|context`.
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    /// Cache time-to-live in seconds.
    cache_ttl_secs: u64,
}

impl ClawzGovernanceEngine {
    /// Creates a new engine with the supplied [`GovernanceEngineConfig`].
    ///
    /// All subsystems are initialised with their own default states; policies,
    /// trust scores, and audit history start empty. The default cache TTL is
    /// **5 seconds**.
    pub fn new(config: GovernanceEngineConfig) -> Self {
        Self::new_with_approval(config, Arc::new(ApprovalWorkflow::new()))
    }

    /// Create an engine using a shared approval workflow (gateway + worker).
    pub fn new_with_approval(
        config: GovernanceEngineConfig,
        approval_workflow: Arc<ApprovalWorkflow>,
    ) -> Self {
        Self {
            config,
            policy_engine: Arc::new(RwLock::new(PolicyEngine::new())),
            trust_scorer: Arc::new(TrustScorer::new()),
            guardrails: Arc::new(RwLock::new(GovernanceGuardrails::new())),
            approval_workflow,
            audit_logger: Arc::new(AuditLogger::new()),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl_secs: 5,
        }
    }

    /// Registers a new [`GovernancePolicy`] with the internal [`PolicyEngine`].
    ///
    /// The policy takes effect on the next call to [`evaluate`](GovernanceEngine::evaluate).
    pub async fn add_policy(&self, policy: GovernancePolicy) {
        self.policy_engine.write().await.add_policy(policy);
    }

    /// Returns a reference to the underlying [`AuditLogger`].
    ///
    /// Useful for inspection in tests and for querying recent history before
    /// handing off to guardrails.
    pub fn audit_logger(&self) -> &AuditLogger {
        &self.audit_logger
    }

    /// Returns a reference to the underlying [`ApprovalWorkflow`].
    pub fn approval_workflow(&self) -> &ApprovalWorkflow {
        &self.approval_workflow
    }

    /// Builds a deterministic cache key from the evaluation inputs.
    ///
    /// The key format is `agent_id|action|context` where `context` is the
    /// JSON serialisation of the value.
    fn cache_key(agent_id: &str, action: &str, context: &serde_json::Value) -> String {
        format!("{agent_id}|{action}|{context}")
    }

    /// Looks up a cached result by key, returning `None` if missing or expired.
    async fn get_cached(&self, key: &str) -> Option<GovernanceResult> {
        let cache = self.cache.read().await;
        if let Some(entry) = cache.get(key) {
            if entry.expires_at > std::time::Instant::now() {
                return Some(entry.result.clone());
            }
        }
        None
    }

    /// Stores a result in the cache with the engine's configured TTL.
    async fn set_cached(&self, key: String, result: GovernanceResult) {
        let expires_at =
            std::time::Instant::now() + std::time::Duration::from_secs(self.cache_ttl_secs);
        self.cache
            .write()
            .await
            .insert(key, CacheEntry { result, expires_at });
    }
}

// Dependency: `clawz_core::traits::GovernanceEngine` trait definition.
#[async_trait]
impl GovernanceEngine for ClawzGovernanceEngine {
    /// Evaluates whether `agent_id` may perform `action` in the given JSON `context`.
    ///
    /// # Orchestration flow
    ///
    /// 1. **Cache lookup** – short-circuit if an identical evaluation was recently
    ///    performed and cached.
    /// 2. **Trust hard deny** – agents with a trust score at or below
    ///    [`deny_below_trust`](GovernanceEngineConfig::deny_below_trust) are
    ///    rejected immediately.
    /// 3. **Policy checks** – every registered policy is evaluated against the
    ///    action and context. `Deny` effects become violations; `Review` effects
    ///    set `requires_review = true`.
    /// 4. **Guardrail checks** — the action string is audited against recent history
    ///    across all guardrail checks. Failures are added as violations.
    /// 5. **Trust review gate** – agents with a trust score below
    ///    [`min_trust_score`](GovernanceEngineConfig::min_trust_score) (but above
    ///    the hard deny) are escalated to `pending_review`.
    /// 6. **Result construction** –
    ///    * Violations present → [`GovernanceResult::deny`].
    ///    * `requires_review` → [`GovernanceResult::pending_review`].
    ///    * Otherwise → [`GovernanceResult::allow`].
    /// 7. **Audit & cache** – the outcome is logged via [`AuditLogger`] and cached.
    ///
    /// # Errors
    ///
    /// Returns [`ClawzError`] only if an underlying subsystem fails (e.g., the
    /// audit logger encounters an I/O error). Most business-logic outcomes are
    /// expressed through [`GovernanceResult`] rather than `Err`.
    async fn evaluate(
        &self,
        agent_id: &str,
        action: &str,
        context: &serde_json::Value,
    ) -> Result<GovernanceResult> {
        let key = Self::cache_key(agent_id, action, context);

        // 1. Check cache.
        if let Some(cached) = self.get_cached(&key).await {
            return Ok(cached);
        }

        let trust_score = self.trust_scorer.get_score(agent_id).await;
        let trust_f64 = trust_score.as_f64();

        // 2. Hard deny below minimum trust.
        if self.config.enable_trust_check && trust_score.score < self.config.deny_below_trust {
            let result = GovernanceResult::deny(
                vec![format!(
                    "trust score {} is below hard deny threshold {}",
                    trust_score.score, self.config.deny_below_trust
                )],
                trust_f64,
            );
            if self.config.audit_enabled {
                self.audit_logger.append(
                    agent_id,
                    action,
                    AuditResult::Deny,
                    serde_json::json!({ "reason": "trust_too_low", "score": trust_score.score }),
                );
            }
            self.set_cached(key, result.clone()).await;
            return Ok(result);
        }

        let mut violations: Vec<String> = Vec::new();
        let mut requires_review = false;

        // 3. Policy checks.
        if self.config.enable_policy_check {
            let engine = self.policy_engine.read().await;
            let policy_violations = engine.evaluate(action, context);
            for v in &policy_violations {
                match v.effect {
                    clawz_core::types::governance::PolicyEffect::Deny => {
                        violations.push(format!(
                            "policy '{}': {}",
                            v.policy_name, v.rule_description
                        ));
                    }
                    clawz_core::types::governance::PolicyEffect::Review => {
                        requires_review = true;
                    }
                    _ => {}
                }
            }
        }

        // 4. Guardrail checks on the action string.
        if self.config.enable_guardrails {
            let guardrails = self.guardrails.read().await;
            // Dependency: `crate::governance::audit::AuditLogger::entries_last_hour`
            let audit_entries = self.audit_logger.entries_last_hour();
            let guardrail_result = guardrails.run_all(action, audit_entries);
            for fail in guardrail_result.failed_dimensions() {
                violations.push(format!(
                    "guardrail {} failed: {}",
                    fail.dimension,
                    fail.details.join(", ")
                ));
            }
        }

        // 5. Trust threshold for review.
        if self.config.enable_trust_check && trust_score.score < self.config.min_trust_score {
            requires_review = true;
        }

        // 6. Construct result.
        let result = if !violations.is_empty() {
            GovernanceResult::deny(violations, trust_f64)
        } else if requires_review {
            GovernanceResult::pending_review(trust_f64, 1)
        } else {
            GovernanceResult::allow(trust_f64)
        };

        // 7. Audit & cache.
        if self.config.audit_enabled {
            let audit_result = if result.allowed {
                AuditResult::Allow
            } else if result.required_approvals > 0 {
                AuditResult::Review
            } else {
                AuditResult::Deny
            };
            self.audit_logger.append(
                agent_id,
                action,
                audit_result,
                serde_json::json!({
                    "trust_score": trust_f64,
                    "violations": result.violations.len(),
                }),
            );
        }

        self.set_cached(key, result.clone()).await;
        Ok(result)
    }

    /// Returns the current trust score for `agent_id` as a normalised `f64`
    /// in the range `0.0` – `1.0`.
    ///
    /// The underlying [`TrustScorer`] stores scores on a 0–1000 integer scale;
    /// this method converts that value for external consumption.
    async fn get_trust_score(&self, agent_id: &str) -> Result<f64> {
        Ok(self.trust_scorer.get_score(agent_id).await.as_f64())
    }

    /// Adjusts the trust score of `agent_id` by `delta` and records the reason.
    ///
    /// `delta` is expected in the `0.0` – `1.0` range and is internally scaled
    /// to the `0` – `1000` integer scale used by [`TrustScorer`].
    async fn update_trust(&self, agent_id: &str, delta: f64, reason: &str) -> Result<()> {
        // Convert 0-1 delta to 0-1000 scale.
        let scaled = (delta * 100.0) as i32;
        self.trust_scorer
            .update_score(agent_id, scaled, reason)
            .await;
        Ok(())
    }

    /// Checks whether `action` is allowed under the specific policy identified by
    /// `policy_id`.
    ///
    /// Returns `Ok(true)` if the policy is disabled (effectively a no-op).
    /// Returns `Ok(false)` if a `Deny` rule in the policy matches the action.
    /// Returns `Err(ClawzError::NotFound)` if the policy does not exist.
    async fn check_policy(&self, policy_id: &str, action: &str) -> Result<bool> {
        let engine = self.policy_engine.read().await;
        match engine.get_policy(policy_id) {
            Some(policy) => {
                if !policy.enabled {
                    return Ok(true); // disabled = allow
                }
                // Check if any Deny rule in the policy matches.
                let violations = engine.evaluate(action, &serde_json::Value::Null);
                let denied = violations.iter().any(|v| {
                    v.policy_id == policy_id
                        && matches!(v.effect, clawz_core::types::governance::PolicyEffect::Deny)
                });
                Ok(!denied)
            }
            None => Err(ClawzError::NotFound {
                entity: "governance policy".into(),
                id: policy_id.into(),
            }),
        }
    }

    /// Submits an approval request through the internal [`ApprovalWorkflow`].
    ///
    /// Returns the unique request identifier on success.
    async fn request_approval(&self, request: ApprovalRequest) -> Result<String> {
        let id = self
            .approval_workflow
            .request(
                &request.agent_id,
                &request.action,
                request.context.clone(),
                request.required_approvals,
            )
            .await;
        Ok(id)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // Dependency: items under test from the parent module.
    use super::*;
    // Dependency: `clawz_core::types::governance::GovernancePolicy` for test fixtures.
    use clawz_core::types::governance::GovernancePolicy;

    /// Helper: build a default engine for unit tests.
    fn make_engine() -> ClawzGovernanceEngine {
        ClawzGovernanceEngine::new(GovernanceEngineConfig::default())
    }

    #[tokio::test]
    async fn test_allow_by_default() {
        let engine = make_engine();
        let result = engine
            .evaluate("agent-1", "chat", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(result.allowed);
    }

    #[tokio::test]
    async fn test_deny_policy_blocks() {
        let engine = make_engine();
        engine
            .add_policy(GovernancePolicy::deny_all("block-all"))
            .await;
        let result = engine
            .evaluate("agent-1", "anything", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(!result.allowed);
        assert!(!result.violations.is_empty());
    }

    #[tokio::test]
    async fn test_trust_score_default() {
        let engine = make_engine();
        let score = engine.get_trust_score("agent-1").await.unwrap();
        assert!((score - 0.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_update_trust() {
        let engine = make_engine();
        engine
            .update_trust("agent-1", 0.1, "good work")
            .await
            .unwrap();
        let score = engine.get_trust_score("agent-1").await.unwrap();
        assert!(score > 0.5);
    }

    #[tokio::test]
    async fn test_low_trust_requires_review() {
        let engine = ClawzGovernanceEngine::new(GovernanceEngineConfig {
            min_trust_score: 600,
            deny_below_trust: 100,
            ..Default::default()
        });
        // Default score is 500 < 600 min → should require approval.
        let result = engine
            .evaluate("agent-1", "chat", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(!result.allowed);
        assert!(result.required_approvals > 0);
    }

    #[tokio::test]
    async fn test_hard_deny_below_threshold() {
        let engine = ClawzGovernanceEngine::new(GovernanceEngineConfig {
            deny_below_trust: 600,
            ..Default::default()
        });
        // Default score is 500 < 600 → hard deny.
        let result = engine
            .evaluate("agent-1", "chat", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(!result.allowed);
        assert!(result.required_approvals == 0); // hard deny, not review
    }

    #[tokio::test]
    async fn test_audit_entries_recorded() {
        let engine = make_engine();
        engine
            .evaluate("agent-1", "chat", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(engine.audit_logger().entry_count(), 1);
    }

    #[tokio::test]
    async fn test_request_approval() {
        let engine = make_engine();
        let req = ApprovalRequest::new("agent-1", "deploy", serde_json::json!({}), 1);
        let id = engine.request_approval(req).await.unwrap();
        assert!(!id.is_empty());
    }
}
