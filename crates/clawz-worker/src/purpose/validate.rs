//! Validate [`GoalObject`] instances for structural correctness.
//!
//! The validator enforces invariants such as non-empty descriptions,
//! normalized reward weights, and present success criteria.

use clawz_core::types::GoalObject;
use clawz_core::error::{ClawzError, Result};

/// Structural validator for parsed goals.
#[derive(Debug, Clone)]
pub struct Validator;

impl Validator {
    /// Create a new validator.
    pub fn new() -> Self {
        Self
    }

    /// Validate a [`GoalObject`].
    ///
    /// # Checks
    /// - Description is non-empty.
    /// - At least one success criterion is present.
    /// - Reward weights sum to 1.0 (within epsilon 0.01).
    /// - All milestone names are non-empty.
    ///
    /// # Errors
    /// Returns [`ClawzError::Validation`] with a descriptive message on failure.
    ///
    /// # Examples
    /// ```
    /// use clawz_core::types::{GoalObject, SuccessCriterion, Comparator, RewardWeights};
    /// use clawz_worker::purpose::Validator;
    ///
    /// let goal = GoalObject::builder()
    ///     .description("test")
    ///     .success_criteria(vec![SuccessCriterion::new("x", Comparator::GreaterThan, serde_json::json!(1))])
    ///     .reward_weights(RewardWeights { accuracy: 1.0, speed: 0.0, cost: 0.0, safety: 0.0 })
    ///     .build();
    ///
    /// assert!(Validator::new().validate(&goal).is_ok());
    /// ```
    pub fn validate(&self, goal: &GoalObject) -> Result<()> {
        if goal.description.trim().is_empty() {
            return Err(ClawzError::Validation(
                "goal description must not be empty".into(),
            ));
        }

        if goal.success_criteria.is_empty() {
            return Err(ClawzError::Validation(
                "goal must have at least one success criterion".into(),
            ));
        }

        if !goal.reward_weights.is_normalized(0.01) {
            return Err(ClawzError::Validation(
                format!(
                    "reward weights must be non-negative and sum to 1.0, got ({:.2}, {:.2}, {:.2}, {:.2})",
                    goal.reward_weights.accuracy,
                    goal.reward_weights.speed,
                    goal.reward_weights.cost,
                    goal.reward_weights.safety,
                ),
            ));
        }

        for ms in &goal.milestones {
            if ms.name.trim().is_empty() {
                return Err(ClawzError::Validation(
                    "milestone name must not be empty".into(),
                ));
            }
        }

        Ok(())
    }
}

impl Default for Validator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::{Comparator, GoalObject, RewardWeights, SuccessCriterion};

    fn valid_goal() -> GoalObject {
        GoalObject::builder()
            .description("reduce cloud spend")
            .success_criteria(vec![SuccessCriterion::new(
                "cost",
                Comparator::LessThan,
                serde_json::json!(5000),
            )])
            .reward_weights(RewardWeights {
                accuracy: 1.0,
                speed: 0.0,
                cost: 0.0,
                safety: 0.0,
            })
            .build()
    }

    #[test]
    fn validate_ok_for_well_formed_goal() {
        let v = Validator::new();
        assert!(v.validate(&valid_goal()).is_ok());
    }

    #[test]
    fn validate_fails_on_empty_description() {
        let v = Validator::new();
        let mut goal = valid_goal();
        goal.description = "".into();
        assert!(v.validate(&goal).is_err());
    }

    #[test]
    fn validate_fails_on_missing_success_criteria() {
        let v = Validator::new();
        let mut goal = valid_goal();
        goal.success_criteria.clear();
        assert!(v.validate(&goal).is_err());
    }

    #[test]
    fn validate_fails_on_unnormalized_weights() {
        let v = Validator::new();
        let mut goal = valid_goal();
        goal.reward_weights = RewardWeights {
            accuracy: 0.5,
            speed: 0.5,
            cost: 0.5,
            safety: 0.5,
        };
        assert!(v.validate(&goal).is_err());
    }

    #[test]
    fn validate_fails_on_empty_milestone_name() {
        let v = Validator::new();
        let mut goal = valid_goal();
        goal.milestones.push(clawz_core::types::Milestone::new(""));
        assert!(v.validate(&goal).is_err());
    }
}
