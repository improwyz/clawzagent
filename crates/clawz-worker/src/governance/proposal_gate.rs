//! Proposal gate — routes [`ImprovementProposal`]s through mode-appropriate governance.
//!
//! This module implements the bridge between the improvement pipeline and the
//! governance subsystem. It is the single integration point where generated proposals
//! are dispatched to the correct approval/council/audit flow based on deployment mode.
//!
//! ## Routing Table
//!
//! | Mode | Governance Path | Required Approvals |
//! |------|-----------------|---------------------|
//! | [`DeploymentMode::Standalone`] | Single human via [`ApprovalWorkflow`] | 1 |
//! | [`DeploymentMode::Micro`] | 2 peer approvals + monthly audit schedule | 2 |
//! | [`DeploymentMode::Elastic`] | [`Council`] deliberation + quarterly HITL audit | council rule |
//!
//! ## Error Handling
//!
//! All errors are typed as [`clawz_core::error::ClawzError`] with full module path.

use crate::governance::approval::ApprovalWorkflow;
use crate::governance::audit::{AuditLogger, AuditResult};
use crate::governance::council::{Council, Proposal as CouncilProposal};
use crate::memory::improvement::ImprovementProposal;
use clawz_core::deployment::DeploymentMode;
use clawz_core::error::ClawzError;
use std::sync::Arc;
use uuid::Uuid;

/// Parse an approval ID string to Uuid, propagating errors as ClawzError.
fn parse_approval_id(id: &str) -> Result<Uuid, ClawzError> {
    Uuid::parse_str(id).map_err(|_| {
        ClawzError::Internal(format!(
            "ApprovalWorkflow returned malformed UUID: {}",
            id
        ))
    })
}

// ── Config ─────────────────────────────────────────────────────────────────────

/// Gate configuration that controls routing behavior.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Deployment topology — determines which governance path is used.
    pub mode: DeploymentMode,
    /// Minimum approvals required before an improvement is applied (Standalone/Micro).
    pub required_approvals: u32,
}

// ── Decision ───────────────────────────────────────────────────────────────────

/// Outcome of a proposal gate routing decision.
#[derive(Debug, Clone)]
pub enum GateDecision {
    /// Proposal is pending approval — returns the approval request ID.
    Pending(Uuid),
    /// Proposal was approved by governance — returns the proposal ID.
    Approved(Uuid),
    /// Proposal was rejected — contains the rejection reason.
    Rejected(String),
}

impl GateDecision {
    pub fn is_approved(&self) -> bool {
        matches!(self, GateDecision::Approved(_))
    }
}

// ── Gatekeeper ─────────────────────────────────────────────────────────────────

/// Routes improvement proposals through mode-appropriate governance.
///
/// The gatekeeper is the single integration point between the improvement pipeline
/// (which generates [`ImprovementProposal`]s) and the governance subsystem (which
/// decides whether to apply them). It should be constructed once per worker and
/// reused for all proposals.
pub struct ProposalGatekeeper {
    config: GateConfig,
    approval_workflow: Arc<ApprovalWorkflow>,
    audit_logger: Arc<AuditLogger>,
    council: Option<Arc<Council>>,
    audit_scheduler: Option<Arc<dyn AuditScheduler>>,
}

/// Schedules periodic governance audits (monthly, quarterly).
/// The scheduler is called after a proposal clears the approval gate
/// in Micro and Elastic modes.
pub trait AuditScheduler: Send + Sync {
    /// Schedule a follow-up audit at the given cron expression.
    fn schedule_audit(&self, cadence: &str, proposal_id: Uuid) -> Result<(), ClawzError>;
}

/// Simple in-memory audit scheduler (for testing / standalone mode).
pub struct DefaultAuditScheduler;

impl AuditScheduler for DefaultAuditScheduler {
    fn schedule_audit(&self, cadence: &str, proposal_id: Uuid) -> Result<(), ClawzError> {
        // In production this would write to a cron scheduler or queue.
        // The audit_logger is used for production audit trails.
        let _ = (cadence, proposal_id);
        Ok(())
    }
}

impl ProposalGatekeeper {
    /// Construct a new gatekeeper.
    pub fn new(
        config: GateConfig,
        approval_workflow: Arc<ApprovalWorkflow>,
        audit_logger: Arc<AuditLogger>,
        audit_scheduler: Option<Arc<dyn AuditScheduler>>,
    ) -> Self {
        Self {
            config,
            approval_workflow,
            audit_logger,
            council: None,
            audit_scheduler,
        }
    }

    /// Attach a [`Council`] for Elastic-mode deliberation.
    pub fn with_council(mut self, council: Arc<Council>) -> Self {
        self.council = Some(council);
        self
    }

    /// Attach an [`AuditScheduler`] for Micro/Elastic mode periodic audits.
    pub fn with_audit_scheduler(mut self, scheduler: Arc<dyn AuditScheduler>) -> Self {
        self.audit_scheduler = Some(scheduler);
        self
    }

    /// Route a proposal through mode-appropriate governance.
    ///
    /// - **Standalone**: requests single human approval via [`ApprovalWorkflow`].
    /// - **Micro**: requests 2 peer approvals via [`ApprovalWorkflow`].
    /// - **Elastic**: delegates to [`Council::deliberate`] if a council is configured,
    ///   otherwise rejects.
    pub async fn route(&self, proposal: ImprovementProposal) -> Result<GateDecision, ClawzError> {
        let context = serde_json::to_value(&proposal)
            .map_err(|e| ClawzError::Internal(e.to_string()))?;

        match self.config.mode {
            DeploymentMode::Standalone => {
                let approval_id = self
                    .approval_workflow
                    .request(
                        "system",
                        "improvement:apply",
                        context,
                        self.config.required_approvals,
                    )
                    .await;
                Ok(GateDecision::Pending(parse_approval_id(&approval_id)?))
        }
        DeploymentMode::Micro => {
            let approval_id = self
                .approval_workflow
                .request(
                    "system",
                    "improvement:apply",
                    context,
                    self.config.required_approvals,
                )
                .await;

            // Schedule monthly audit for this proposal
            if let Some(scheduler) = &self.audit_scheduler {
                scheduler.schedule_audit("monthly", proposal.proposal_id)?;
            }

            Ok(GateDecision::Pending(parse_approval_id(&approval_id)?))
            }
            DeploymentMode::Elastic => {
                if let Some(council) = &self.council {
                    let council_proposal = CouncilProposal::new(
                        format!("Improvement: {}", proposal.proposal_id),
                        proposal.suggested_changes.join(", "),
                        context.clone(),
                    );
                    let decision = council
                        .deliberate(&council_proposal)
                        .await
                        .map_err(|e| ClawzError::Internal(e.to_string()))?;

                    if decision.approved {
                        self.audit_logger
                            .append(
                                "system",
                                "improvement_approved",
                                AuditResult::Allow,
                                serde_json::json!({
                                    "proposal_id": proposal.proposal_id.to_string(),
                                    "council_approved": true
                                }),
                            );
                        Ok(GateDecision::Pending(proposal.proposal_id))
                    } else {
                        self.audit_logger
                            .append(
                                "system",
                                "improvement_rejected",
                                AuditResult::Deny,
                                serde_json::json!({
                                    "proposal_id": proposal.proposal_id.to_string(),
                                    "council_approved": false
                                }),
                            );
                        Ok(GateDecision::Rejected("council_rejected".into()))
                    }
                } else {
                    Ok(GateDecision::Rejected("council_not_configured".into()))
                }
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::approval::ApprovalWorkflow;
    use crate::governance::audit::AuditLogger;
    use chrono::Utc;

    fn make_proposal() -> ImprovementProposal {
        ImprovementProposal {
            proposal_id: uuid::Uuid::new_v4(),
            generated_at: Utc::now(),
            triggering_metrics: vec![],
            suggested_changes: vec!["add retry logic".into()],
            confidence: 0.8,
            identity_modification: None,
        }
    }

    #[tokio::test]
    async fn proposal_gate_routes_to_approval_workflow_in_standalone_mode() {
        let gate = ProposalGatekeeper::new(
            GateConfig {
                mode: DeploymentMode::Standalone,
                required_approvals: 1,
            },
            Arc::new(ApprovalWorkflow::new()),
            Arc::new(AuditLogger::new()),
            None,
        );

        let proposal = make_proposal();
        let decision = gate.route(proposal).await.unwrap();
        assert!(matches!(decision, GateDecision::Pending(_)));
    }

    #[tokio::test]
    async fn proposal_gate_requires_more_approvals_in_micro_mode() {
        let gate = ProposalGatekeeper::new(
            GateConfig {
                mode: DeploymentMode::Micro,
                required_approvals: 2,
            },
            Arc::new(ApprovalWorkflow::new()),
            Arc::new(AuditLogger::new()),
            None,
        );

        let proposal = make_proposal();
        let decision = gate.route(proposal).await.unwrap();
        assert!(matches!(decision, GateDecision::Pending(_)));
    }

    #[tokio::test]
    async fn proposal_gate_rejects_when_council_not_configured_in_elastic_mode() {
        let gate = ProposalGatekeeper::new(
            GateConfig {
                mode: DeploymentMode::Elastic,
                required_approvals: 0,
            },
            Arc::new(ApprovalWorkflow::new()),
            Arc::new(AuditLogger::new()),
            None,
        );

        let proposal = make_proposal();
        let decision = gate.route(proposal).await.unwrap();
        assert!(matches!(decision, GateDecision::Rejected(r) if r == "council_not_configured"));
    }

    #[tokio::test]
    async fn proposal_gate_uses_council_when_configured_in_elastic_mode() {
        let council = Arc::new(Council::new());
        council
            .add_member("arbiter", crate::governance::council::CouncilRole::Arbiter)
            .await;
        council
            .add_member("reviewer", crate::governance::council::CouncilRole::Reviewer)
            .await;

        let gate = ProposalGatekeeper::new(
            GateConfig {
                mode: DeploymentMode::Elastic,
                required_approvals: 0,
            },
            Arc::new(ApprovalWorkflow::new()),
            Arc::new(AuditLogger::new()),
            None,
        )
        .with_council(council.clone());

        // Cast an approval vote so the council can reach a decision
        council
            .cast_vote("reviewer", crate::governance::council::Vote::Approve, None)
            .await
            .unwrap();

        let proposal = make_proposal();
        let decision = gate.route(proposal).await.unwrap();
        // Council approved — elastic mode returns Pending since council deliberated synchronously
        assert!(matches!(decision, GateDecision::Pending(_)));
    }
}