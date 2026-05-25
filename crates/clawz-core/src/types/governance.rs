//! Governance types — policies, trust tiers, and approval workflows.
//!
//! The governance engine evaluates CEL-like conditions against every
//! agent action and produces a `GovernanceResult`. The result may
//! require human approval before the action can proceed.
//!
//! // Dependency: used by worker::governance_engine, gateway::approval_handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── PolicyRule ────────────────────────────────────────────────────────────────

/// Outcome of a single policy rule evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyEffect {
    Allow,
    Deny,
    Review,
}

impl std::fmt::Display for PolicyEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyEffect::Allow => write!(f, "allow"),
            PolicyEffect::Deny => write!(f, "deny"),
            PolicyEffect::Review => write!(f, "review"),
        }
    }
}

/// A single rule inside a governance policy.
/// // Dependency: evaluated by worker::governance_engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// CEL-like condition expression evaluated at runtime.
    /// Example: `"action == 'deploy' && env == 'production'"`
    pub condition: String,
    pub effect: PolicyEffect,
    pub description: String,
}

impl PolicyRule {
    pub fn allow(condition: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            condition: condition.into(),
            effect: PolicyEffect::Allow,
            description: description.into(),
        }
    }

    pub fn deny(condition: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            condition: condition.into(),
            effect: PolicyEffect::Deny,
            description: description.into(),
        }
    }

    pub fn review(condition: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            condition: condition.into(),
            effect: PolicyEffect::Review,
            description: description.into(),
        }
    }
}

// ── GovernancePolicy ──────────────────────────────────────────────────────────

/// A named collection of rules with a priority order.
/// // Dependency: loaded by worker::governance_engine from db or config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernancePolicy {
    pub id: String,
    pub name: String,
    pub rules: Vec<PolicyRule>,
    pub enabled: bool,
    /// Higher priority policies are evaluated first.
    pub priority: i32,
}

impl GovernancePolicy {
    pub fn new(id: impl Into<String>, name: impl Into<String>, rules: Vec<PolicyRule>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            rules,
            enabled: true,
            priority: 0,
        }
    }

    /// Create a simple deny-all policy.
    pub fn deny_all(name: impl Into<String>) -> Self {
        Self::new(
            Uuid::new_v4().to_string(),
            name,
            vec![PolicyRule::deny("true", "deny all actions by default")],
        )
    }

    /// Create a simple allow-all policy.
    pub fn allow_all(name: impl Into<String>) -> Self {
        Self::new(
            Uuid::new_v4().to_string(),
            name,
            vec![PolicyRule::allow("true", "allow all actions")],
        )
    }
}

// ── GovernanceResult ──────────────────────────────────────────────────────────

/// Outcome of a governance evaluation — allow, deny, or require review.
/// // Dependency: returned by traits::GovernanceEngine::evaluate, consumed by worker::pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceResult {
    pub allowed: bool,
    /// Human-readable descriptions of triggered rule violations.
    pub violations: Vec<String>,
    /// Normalised score from 0.0 (fully untrusted) to 1.0 (fully trusted).
    pub trust_score: f64,
    /// Number of human approvals required before this action may proceed.
    pub required_approvals: u32,
    /// The trust tier derived from `trust_score`.
    pub tier: TrustTier,
}

impl GovernanceResult {
    pub fn allow(trust_score: f64) -> Self {
        Self {
            allowed: true,
            violations: Vec::new(),
            trust_score,
            required_approvals: 0,
            tier: TrustTier::from_score(trust_score),
        }
    }

    pub fn deny(violations: Vec<String>, trust_score: f64) -> Self {
        Self {
            allowed: false,
            violations,
            trust_score,
            required_approvals: 0,
            tier: TrustTier::from_score(trust_score),
        }
    }

    pub fn pending_review(trust_score: f64, approvals: u32) -> Self {
        Self {
            allowed: false,
            violations: Vec::new(),
            trust_score,
            required_approvals: approvals,
            tier: TrustTier::from_score(trust_score),
        }
    }
}

// ── TrustTier ─────────────────────────────────────────────────────────────────

/// Discrete trust levels derived from the continuous `trust_score`.
/// // Dependency: used by worker::governance to decide capability grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustTier {
    /// Score 0.0 – 0.19: completely untrusted, no capabilities.
    Untrusted,
    /// Score 0.2 – 0.39: limited read-only capabilities.
    Limited,
    /// Score 0.4 – 0.59: standard user-level capabilities.
    Standard,
    /// Score 0.6 – 0.79: trusted with most operations.
    Trusted,
    /// Score 0.8 – 1.0: full capabilities including privileged ops.
    Full,
}

impl TrustTier {
    pub fn from_score(score: f64) -> Self {
        match score {
            s if s < 0.2 => TrustTier::Untrusted,
            s if s < 0.4 => TrustTier::Limited,
            s if s < 0.6 => TrustTier::Standard,
            s if s < 0.8 => TrustTier::Trusted,
            _ => TrustTier::Full,
        }
    }

    pub fn min_score(&self) -> f64 {
        match self {
            TrustTier::Untrusted => 0.0,
            TrustTier::Limited => 0.2,
            TrustTier::Standard => 0.4,
            TrustTier::Trusted => 0.6,
            TrustTier::Full => 0.8,
        }
    }
}

impl std::fmt::Display for TrustTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrustTier::Untrusted => write!(f, "untrusted"),
            TrustTier::Limited => write!(f, "limited"),
            TrustTier::Standard => write!(f, "standard"),
            TrustTier::Trusted => write!(f, "trusted"),
            TrustTier::Full => write!(f, "full"),
        }
    }
}

// ── ApprovalRequest / ApprovalStatus ──────────────────────────────────────────

/// Lifecycle of a human-approval ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl std::fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApprovalStatus::Pending => write!(f, "pending"),
            ApprovalStatus::Approved => write!(f, "approved"),
            ApprovalStatus::Rejected => write!(f, "rejected"),
            ApprovalStatus::Expired => write!(f, "expired"),
        }
    }
}

/// Human approval ticket for a governance-denied action.
/// // Dependency: created by worker::governance, surfaced via gateway::approval_handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub agent_id: String,
    pub action: String,
    pub context: serde_json::Value,
    pub required_approvals: u32,
    pub granted_approvals: u32,
    pub status: ApprovalStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub approvers: Vec<String>,
}

impl ApprovalRequest {
    pub fn new(
        agent_id: impl Into<String>,
        action: impl Into<String>,
        context: serde_json::Value,
        required_approvals: u32,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            action: action.into(),
            context,
            required_approvals,
            granted_approvals: 0,
            status: ApprovalStatus::Pending,
            created_at: Utc::now(),
            expires_at: None,
            approvers: Vec::new(),
        }
    }

    pub fn with_expiry(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Record an approval from `approver_id`. Returns `true` if now fully approved.
    pub fn approve(&mut self, approver_id: impl Into<String>) -> bool {
        if self.status != ApprovalStatus::Pending {
            return false;
        }
        let approver = approver_id.into();
        if !self.approvers.contains(&approver) {
            self.approvers.push(approver);
            self.granted_approvals += 1;
        }
        if self.granted_approvals >= self.required_approvals {
            self.status = ApprovalStatus::Approved;
            return true;
        }
        false
    }

    pub fn reject(&mut self) {
        self.status = ApprovalStatus::Rejected;
    }

    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires_at {
            Utc::now() > exp
        } else {
            false
        }
    }
}

// ── OversightLevel ────────────────────────────────────────────────────────────

/// PRISM-G Volume 9 — the five levels of human oversight, from most to least
/// autonomous. Used by the Governance dimension to gate actions by required
/// human involvement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OversightLevel {
    /// AI acts independently; humans are not in the path.
    Autonomous,
    /// Humans observe and may intervene, but do not pre-approve.
    Monitored,
    /// A human is involved in the decision before the action proceeds.
    HumanInTheLoop,
    /// The human decides; the AI only assists.
    HumanDirected,
    /// The human performs the action; the AI only observes.
    Manual,
}

impl OversightLevel {
    /// All five levels, most→least autonomous.
    pub const ALL: [OversightLevel; 5] = [
        OversightLevel::Autonomous,
        OversightLevel::Monitored,
        OversightLevel::HumanInTheLoop,
        OversightLevel::HumanDirected,
        OversightLevel::Manual,
    ];

    /// Higher = more autonomous. Autonomous=4 … Manual=0.
    pub fn autonomy_rank(&self) -> u8 {
        match self {
            OversightLevel::Autonomous => 4,
            OversightLevel::Monitored => 3,
            OversightLevel::HumanInTheLoop => 2,
            OversightLevel::HumanDirected => 1,
            OversightLevel::Manual => 0,
        }
    }

    /// True when a human must take or approve the decision before execution.
    pub fn requires_human_decision(&self) -> bool {
        matches!(
            self,
            OversightLevel::HumanInTheLoop
                | OversightLevel::HumanDirected
                | OversightLevel::Manual
        )
    }
}

#[cfg(test)]
mod oversight_tests {
    use super::OversightLevel;

    #[test]
    fn five_levels_ordered_by_autonomy() {
        assert!(
            OversightLevel::Autonomous.autonomy_rank() > OversightLevel::Manual.autonomy_rank()
        );
        assert_eq!(OversightLevel::ALL.len(), 5);
    }

    #[test]
    fn human_in_the_loop_requires_human_decision() {
        assert!(OversightLevel::HumanInTheLoop.requires_human_decision());
        assert!(!OversightLevel::Monitored.requires_human_decision());
    }
}
