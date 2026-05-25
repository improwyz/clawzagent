//! Purpose dimension — structuring vague goals into machine-readable objectives.
//!
//! This module implements the **P** dimension of PRISM-G. It classifies
//! intent, extracts constraints and targets, validates structural
//! invariants, and produces [`GoalObject`]s that the agent runtime can
//! plan against.
//!
//! | Sub-module | Responsibility |
//! |------------|----------------|
//! | [`classify`] | Map natural language to [`GoalType`] |
//! | [`extract`]  | Pull metrics, dates, and targets from free text |
//! | [`validate`] | Enforce structural invariants on [`GoalObject`] |
//! | [`parser`]   | Orchestrate classification → extraction → validation |

pub mod classify;
pub mod extract;
pub mod parser;
pub mod validate;

pub use classify::classify;
pub use extract::Extractor;
pub use parser::GoalParser;
pub use validate::Validator;

// ── PipelineStep integration ──────────────────────────────────────────────────

use async_trait::async_trait;
use clawz_core::{
    error::Result,
    traits::{PipelineContext, PipelineStep, StepOutcome},
    types::message::Role,
};

/// A [`PipelineStep`] that parses a goal from the user message in the
/// pipeline context and stores the resulting [`GoalObject`] in context
/// metadata under the key `"goal_object"`.
///
/// # Step behaviour
/// 1. Read the last user message text from `ctx.messages`.
/// 2. Run it through [`GoalParser::parse`].
/// 3. On [`ParseOutcome::Parsed`], insert the `GoalObject` as JSON metadata.
/// 4. On [`ParseOutcome::NeedsClarification`], insert questions and return
///    [`StepOutcome::Halt`] so the agent can ask the user.
/// 5. On [`ParseOutcome::Failed`], return the error.
///
/// # Metadata keys
/// - `"goal_object"` — the parsed [`serde_json::Value`] of the goal.
/// - `"clarification_questions"` — questions to ask when input is vague.
#[derive(Debug, Clone)]
pub struct GoalIntakeStep {
    parser: GoalParser,
}

impl GoalIntakeStep {
    /// Create a new goal-intake step with the default parser.
    pub fn new() -> Self {
        Self {
            parser: GoalParser::new(),
        }
    }
}

impl Default for GoalIntakeStep {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PipelineStep for GoalIntakeStep {
    fn name(&self) -> &str {
        "goal_intake"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome> {
        // Find the last user message text.
        let text = ctx
            .messages
            .iter()
            .rev()
            .find_map(|m| {
                if m.role == Role::User {
                    m.content.as_text()
                } else {
                    None
                }
            })
            .unwrap_or("");

        match self.parser.parse(text) {
            clawz_core::types::ParseOutcome::Parsed(goal) => {
                let value = serde_json::to_value(goal)
                    .map_err(|e| clawz_core::error::ClawzError::Serialization(e.to_string()))?;
                ctx.insert_meta("goal_object", value);
                Ok(StepOutcome::Continue)
            }
            clawz_core::types::ParseOutcome::NeedsClarification(questions) => {
                let value = serde_json::to_value(questions)
                    .map_err(|e| clawz_core::error::ClawzError::Serialization(e.to_string()))?;
                ctx.insert_meta("clarification_questions", value);
                Ok(StepOutcome::Halt)
            }
            clawz_core::types::ParseOutcome::Failed(reason) => {
                Err(clawz_core::error::ClawzError::Validation(reason))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::traits::PipelineContext;
    use clawz_core::types::Message;

    fn make_ctx_with_user_msg(text: impl Into<String>) -> PipelineContext {
        let mut ctx = PipelineContext::new("agent-1", "conv-1");
        ctx.messages.push(Message::user(text));
        ctx
    }

    #[tokio::test]
    async fn goal_intake_step_parses_goal() {
        let step = GoalIntakeStep::new();
        let mut ctx = make_ctx_with_user_msg("minimize latency under 50ms");
        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));
        assert!(ctx.get_meta("goal_object").is_some());
    }

    #[tokio::test]
    async fn goal_intake_step_halts_on_vague_input() {
        let step = GoalIntakeStep::new();
        let mut ctx = make_ctx_with_user_msg("explore");
        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Halt));
        assert!(ctx.get_meta("clarification_questions").is_some());
    }

    #[tokio::test]
    async fn goal_intake_step_uses_last_user_message() {
        let step = GoalIntakeStep::new();
        let mut ctx = make_ctx_with_user_msg("first request");
        ctx.messages.push(Message::assistant("ok"));
        ctx.messages.push(Message::user("reduce cost below $100"));
        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));
        let goal = ctx.get_meta("goal_object").unwrap();
        let desc = goal.get("description").unwrap().as_str().unwrap();
        assert!(desc.contains("cost") || desc.contains("$100"));
    }
}
