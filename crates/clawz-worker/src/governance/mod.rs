//! Governance subsystem for the clawz worker.
//!
//! This module implements the worker-side governance framework that enforces
//! policies, evaluates trust scores, manages approval workflows, and maintains
//! tamper-evident audit trails for agent actions. It serves as the enforcement
//! layer that translates governance rules into concrete decisions (allow, deny,
//! review) before an agent executes a task.
//!
//! ## Key Components
//!
//! | Module | Responsibility |
//! |--------|--------------| 
//! | [`engine`] | [`ClawzGovernanceEngine`] — orchestrates all checks (policy, guardrails, trust, approval) and produces a final [`clawz_core::types::governance::GovernanceResult`]. |
//! | [`policy`] | [`PolicyEngine`] — pattern-matching rule evaluation with priority ordering. |
//! | [`trust`] | [`TrustScorer`] — score tracking with decay, tier mapping, and historical events. |
//! | [`approval`] | [`ApprovalWorkflow`] — request/approve/reject/escalate lifecycle for human-in-the-loop actions. |
//! | [`audit`] | [`AuditLogger`] — SHA-256 hash-chained audit log for tamper evidence. |
//! | [`guardrails`] | [`GovernanceGuardrails`] — G-dimension runtime safety/compliance enforcement (PRISM-G Vol 9). |
//! | [`oversight`] | Oversight level enforcement — maps `RiskLevel` to `OversightLevel`, determines effective oversight, checks pre-approval requirements. |
//! | [`council`] | [`Council`] — multi-agent deliberation with configurable voting rules (unanimous, majority, supermajority). |
//! | [`compliance`] | [`ComplianceExporter`] — maps audit evidence to external frameworks (SOC2, GDPR, EU AI Act). |
//!
//! ## Dependencies on `clawz-core`
//!
//! The governance subsystem relies heavily on `clawz-core` for shared types and
//! contracts:
//!
//! - **`clawz_core::traits::GovernanceEngine`** — implemented by [`ClawzGovernanceEngine`].
//! - **`clawz_core::types::governance`** — [`clawz_core::types::governance::GovernancePolicy`],
//!   [`clawz_core::types::governance::ApprovalRequest`],
//!   [`clawz_core::types::governance::GovernanceResult`],
//!   [`clawz_core::types::governance::TrustTier`],
//!   [`clawz_core::types::governance::ApprovalStatus`],
//!   [`clawz_core::types::governance::PolicyRule`],
//!   [`clawz_core::types::governance::PolicyEffect`],
//!   [`clawz_core::types::governance::OversightLevel`].
//! - **`clawz_core::error::{ClawzError, Result}`** — uniform error handling across
//!   the worker and core crates.
//!
//! Keeping these types in `clawz-core` ensures that the orchestrator and other
//! workers can consume governance results without coupling to the implementation
//! details in `clawz-worker`.

/// Approval workflows — request, approve, reject, escalate, and expire.
pub mod approval;
/// Audit logging with SHA-256 hash chain for tamper evidence.
pub mod audit;
/// Compliance evidence export — maps audit logs to compliance frameworks.
pub mod compliance;
/// Multi-agent deliberation with voting and tie-breaking.
pub mod council;
/// Governance engine implementation — combines policy, trust, guardrails, and approval checks.
pub mod engine;
/// Governance-dimension runtime guardrails (Vol 9).
pub mod guardrails;
/// Oversight level enforcement for the G dimension (PRISM-G Vol 9).
pub mod oversight;
/// Policy engine — pattern matching and condition evaluation.
pub mod policy;
/// Trust scoring with decay, history, and tier mapping.
pub mod trust;

/// Orchestrates governance checks and produces a final [`clawz_core::types::governance::GovernanceResult`].
///
/// This is the primary entry point for the governance subsystem. It coordinates
/// the [`PolicyEngine`], [`TrustScorer`], [`GovernanceGuardrails`], and [`ApprovalWorkflow`]
/// according to [`GovernanceEngineConfig`].
pub use engine::{ClawzGovernanceEngine, GovernanceEngineConfig};
/// Pattern-matching rule engine that evaluates [`clawz_core::types::governance::GovernancePolicy`]
/// in priority order.
pub use policy::PolicyEngine;
/// Tracks agent trust scores (0–1000) with decay, tier mapping, and historical events.
pub use trust::TrustScorer;
/// Manages the request/approve/reject/escalate lifecycle for human-in-the-loop actions.
pub use approval::ApprovalWorkflow;
/// SHA-256 hash-chained audit logger for tamper-evident records.
pub use audit::AuditLogger;
/// G-dimension runtime guardrails — safety/compliance enforcement for the G dimension (PRISM-G Vol 9).
pub use guardrails::GovernanceGuardrails;
/// G-dimension oversight level enforcement — maps RiskLevel to OversightLevel and checks pre-approval requirements.
pub use oversight::{effective_oversight, minimum_oversight_for_risk, requires_pre_approval};
/// Multi-agent deliberation body with configurable voting rules and tie-breaking.
pub use council::Council;
/// Exports audit evidence mapped to external compliance frameworks (SOC2, GDPR, EU AI Act).
pub use compliance::ComplianceExporter;
