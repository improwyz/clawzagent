//! ApplyGovernanceStep — evaluates the provider response against governance policies.
//!
//! Responsibilities:
//! - Reads the ChatResponse from context metadata (placed there by
//!   [`SelectProviderStep`](crate::runtime::steps::provider::SelectProviderStep)).
//! - Evaluates the action against the [`GovernanceEngine`](clawz_core::traits::GovernanceEngine).
//! - Checks trust score for the agent.
//! - If denied: substitutes a safe decline response and halts.
//! - Records the governance result in context.
//!
//! # Cross-module dependencies
//! - Reads `META_CHAT_RESPONSE` from the provider step's metadata output.
//! - Writes `META_GOVERNANCE_RESULT` and `META_GOVERNANCE_BLOCKED` for
//!   downstream observability and audit.
//! - Uses `clawz_core::traits::GovernanceEngine` for policy evaluation.

use std::sync::Arc;

use async_trait::async_trait;
// Dependency: core error types, governance traits, and message primitives.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{GovernanceEngine, PipelineContext, PipelineStep, StepOutcome},
    types::{
        governance::GovernanceResult,
        message::{Message, Role},
    },
};

// Dependency: metadata key produced by the provider step.
use crate::runtime::steps::provider::META_CHAT_RESPONSE;

/// Metadata key for the governance result.
///
/// Written as a JSON value so audit consumers can deserialize the full
/// `GovernanceResult` without re-evaluating.
pub const META_GOVERNANCE_RESULT: &str = "governance_result";
/// Metadata key: set to `true` if the response was blocked.
pub const META_GOVERNANCE_BLOCKED: &str = "governance_blocked";

/// Safe decline message used when governance denies the response.
///
/// This generic wording is intentionally vague to avoid leaking policy
/// details to end users (security by obscurity for policy names).
const SAFE_DECLINE_MESSAGE: &str =
    "I'm unable to assist with that request due to policy restrictions.";

/// Pipeline step that applies governance policy checks to the current turn.
///
/// Runs after the provider step so it can inspect the model's output before
/// it is persisted or streamed to the user.
pub struct ApplyGovernanceStep {
    /// Shared governance engine (trust scoring, policy evaluation, approval workflows).
    engine: Arc<dyn GovernanceEngine>,
    /// Action identifier passed to the engine (e.g. `"agent_chat"`).
    action: String,
}

impl ApplyGovernanceStep {
    /// Create a new governance step.
    pub fn new(engine: Arc<dyn GovernanceEngine>, action: impl Into<String>) -> Self {
        Self {
            engine,
            action: action.into(),
        }
    }
}

#[async_trait]
impl PipelineStep for ApplyGovernanceStep {
    fn name(&self) -> &str {
        "apply_governance"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome> {
        // Build evaluation context from pipeline metadata.
        // We include the presence/absence of a chat response so the engine
        // can distinguish between tool-call turns (no text to censor) and
        // regular chat turns.
        let eval_context = serde_json::json!({
            "agent_id": ctx.agent_id,
            "conversation_id": ctx.conversation_id,
            "action": self.action,
            "has_response": ctx.get_meta(META_CHAT_RESPONSE).is_some(),
            "room_id": ctx.get_meta("room_id"),
            "sender_user_id": ctx.get_meta("sender_user_id"),
            "orchestration_run_id": ctx.get_meta("orchestration_run_id"),
        });

        let result = self
            .engine
            .evaluate(&ctx.agent_id, &self.action, &eval_context)
            .await
            .map_err(|e| ClawzError::Governance(format!("governance evaluation failed: {e}")))?;

        log::debug!(
            "[governance] agent='{}' action='{}' allowed={} tier={}",
            ctx.agent_id,
            self.action,
            result.allowed,
            result.tier
        );

        if !result.violations.is_empty() {
            log::warn!("[governance] violations: {:?}", result.violations);
        }

        // Store result in both typed context field and metadata for audit.
        ctx.governance_result = Some(result.clone());
        ctx.insert_meta(
            META_GOVERNANCE_RESULT,
            serde_json::to_value(&result).map_err(|e| ClawzError::Serialization(e.to_string()))?,
        );

        if !result.allowed {
            // Replace any assistant response with a safe decline message.
            // We strip the original assistant message so it never reaches
            // persistence or the end user.
            log::info!(
                "[governance] blocking response for agent '{}': {:?}",
                ctx.agent_id,
                result.violations
            );
            ctx.insert_meta(META_GOVERNANCE_BLOCKED, serde_json::Value::Bool(true));

            // Remove any assistant message already appended by SelectProviderStep.
            ctx.messages.retain(|m| m.role != Role::Assistant);

            // Append safe decline so the caller still receives a valid Message.
            ctx.messages.push(Message::assistant(SAFE_DECLINE_MESSAGE));

            return Ok(StepOutcome::Halt);
        }

        Ok(StepOutcome::Continue)
    }
}

// ── No-op governance engine for testing ────────────────────────────────────────

/// Governance engine stub that always allows every request.
///
/// Useful in unit tests where governance policy evaluation is not the
/// subject under test.
pub struct AllowAllGovernance;

#[async_trait]
impl GovernanceEngine for AllowAllGovernance {
    async fn evaluate(
        &self,
        _agent_id: &str,
        _action: &str,
        _context: &serde_json::Value,
    ) -> Result<GovernanceResult> {
        Ok(GovernanceResult::allow(0.8))
    }

    async fn get_trust_score(&self, _agent_id: &str) -> Result<f64> {
        Ok(0.8)
    }

    async fn update_trust(&self, _agent_id: &str, _delta: f64, _reason: &str) -> Result<()> {
        Ok(())
    }

    async fn check_policy(&self, _policy_id: &str, _action: &str) -> Result<bool> {
        Ok(true)
    }

    async fn request_approval(
        &self,
        _request: clawz_core::types::governance::ApprovalRequest,
    ) -> Result<String> {
        Ok(uuid::Uuid::new_v4().to_string())
    }
}

/// Governance engine stub that always denies every request.
///
/// Useful for testing the denial / safe-decline code path.
pub struct DenyAllGovernance;

#[async_trait]
impl GovernanceEngine for DenyAllGovernance {
    async fn evaluate(
        &self,
        _agent_id: &str,
        _action: &str,
        _context: &serde_json::Value,
    ) -> Result<GovernanceResult> {
        Ok(GovernanceResult::deny(
            vec!["policy: deny all".to_string()],
            0.1,
        ))
    }

    async fn get_trust_score(&self, _agent_id: &str) -> Result<f64> {
        Ok(0.1)
    }

    async fn update_trust(&self, _agent_id: &str, _delta: f64, _reason: &str) -> Result<()> {
        Ok(())
    }

    async fn check_policy(&self, _policy_id: &str, _action: &str) -> Result<bool> {
        Ok(false)
    }

    async fn request_approval(
        &self,
        _request: clawz_core::types::governance::ApprovalRequest,
    ) -> Result<String> {
        Ok(uuid::Uuid::new_v4().to_string())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::{traits::PipelineContext, types::message::Message};

    fn make_ctx() -> PipelineContext {
        let mut ctx = PipelineContext::new("agent-1", "conv-1");
        ctx.messages.push(Message::user("hello"));
        ctx.messages.push(Message::assistant("response"));
        ctx
    }

    #[tokio::test]
    async fn test_allow_all_continues() {
        let step = ApplyGovernanceStep::new(Arc::new(AllowAllGovernance), "chat");
        let mut ctx = make_ctx();
        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));
        assert!(ctx.governance_result.as_ref().unwrap().allowed);
    }

    #[tokio::test]
    async fn test_deny_all_halts_and_substitutes() {
        let step = ApplyGovernanceStep::new(Arc::new(DenyAllGovernance), "chat");
        let mut ctx = make_ctx();
        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Halt));
        assert_eq!(
            ctx.get_meta(META_GOVERNANCE_BLOCKED),
            Some(&serde_json::Value::Bool(true))
        );
        // Last message should be the safe decline.
        let last = ctx.messages.last().unwrap();
        assert_eq!(last.role, Role::Assistant);
        assert!(
            last.content
                .as_text()
                .unwrap()
                .contains("policy restrictions")
        );
    }
}
