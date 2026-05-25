//! Parse natural-language goal strings into structured [`GoalObject`]s.
//!
//! `GoalParser` orchestrates classification, extraction, and validation
//! to turn vague human intent into machine-readable goals.

use clawz_core::types::{
    ClarificationQuestion, Comparator, Constraint, ConstraintKind, GoalObject, GoalType,
    Milestone, ParseOutcome, SuccessCriterion,
};

use crate::purpose::{classify, Extractor, Validator};

/// Goal parser that turns natural language into structured goals.
#[derive(Debug, Clone)]
pub struct GoalParser {
    extractor: Extractor,
    validator: Validator,
}

impl GoalParser {
    /// Create a new parser with default extractor and validator.
    pub fn new() -> Self {
        Self {
            extractor: Extractor::new(),
            validator: Validator::new(),
        }
    }

    /// Parse a natural-language goal string.
    ///
    /// # Algorithm
    /// 1. Classify the goal type from keywords.
    /// 2. Extract metrics, dates, and target.
    /// 3. Build a [`GoalObject`] with heuristically-populated fields.
    /// 4. Validate the result.
    /// 5. Return [`ParseOutcome::Parsed`] on success, or
    ///    [`ParseOutcome::NeedsClarification`] if the input is too vague.
    ///
    /// # Examples
    /// ```
    /// use clawz_worker::purpose::GoalParser;
    /// use clawz_core::types::ParseOutcome;
    ///
    /// let parser = GoalParser::new();
    /// let outcome = parser.parse("minimize latency under 100ms by 2025-12-31");
    ///
    /// match outcome {
    ///     ParseOutcome::Parsed(goal) => {
    ///         assert_eq!(goal.goal_type, clawz_core::types::GoalType::Optimize);
    ///         assert!(!goal.description.is_empty());
    ///     }
    ///     ParseOutcome::NeedsClarification(_) => {}
    ///     ParseOutcome::Failed(_) => panic!("unexpected failure"),
    /// }
    /// ```
    pub fn parse(&self, input: &str) -> ParseOutcome {
        if input.trim().is_empty() {
            return ParseOutcome::Failed("input is empty".into());
        }

        let goal_type = classify(input);
        let metrics = self.extractor.extract_metrics(input);
        let dates = self.extractor.extract_dates(input);
        let target = self.extractor.extract_target(input);

        // Build constraints from metrics
        let mut constraints: Vec<Constraint> = Vec::new();
        for (name, value) in &metrics {
            constraints.push(Constraint::new(
                ConstraintKind::Quality,
                format!("{} constraint", name),
                serde_json::json!(value),
            ));
        }

        // Build success criteria from metrics
        let mut criteria: Vec<SuccessCriterion> = Vec::new();
        for (name, value) in &metrics {
            criteria.push(SuccessCriterion::new(
                name.clone(),
                Comparator::LessThan,
                serde_json::json!(value),
            ));
        }

        // If nothing concrete was extracted, ask for clarification
        if criteria.is_empty() && target.len() < 5 {
            return ParseOutcome::NeedsClarification(vec![
                ClarificationQuestion::new(
                    "Could you specify a metric or target?",
                    "no concrete metric found in input",
                ),
            ]);
        }

        // Check if we have concrete material before building (must check before moving)
        let has_concrete_material = !metrics.is_empty() && target.len() >= 5;

        let mut builder = GoalObject::builder()
            .description(target)
            .goal_type(goal_type)
            .constraints(constraints)
            .success_criteria(criteria);

        // Add a milestone if a date was found
        if let Some(due) = dates.first() {
            builder = builder.milestones(vec![Milestone::new("target date").with_due_date(*due)]);
        }

        let goal = builder.build();

        // If validation fails on structural grounds (empty criteria, etc.),
        // treat as needs-clarification rather than a hard failure.
        let validation_result = self.validator.validate(&goal);
        match validation_result {
            Ok(()) => ParseOutcome::Parsed(goal),
            Err(_) if !has_concrete_material => {
                // Structurally incomplete but fixable — ask for clarification.
                ParseOutcome::NeedsClarification(vec![ClarificationQuestion::new(
                    "Could you be more specific about the metric or target?",
                    "goal is structurally valid but missing concrete criteria",
                )])
            }
            Err(e) => ParseOutcome::Failed(e.to_string()),
        }
    }
}

impl Default for GoalParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::GoalType;

    #[test]
    fn parse_empty_input_fails() {
        let parser = GoalParser::new();
        let outcome = parser.parse("");
        assert!(matches!(outcome, ParseOutcome::Failed(_)));
    }

    #[test]
    fn parse_optimize_goal_with_metric() {
        let parser = GoalParser::new();
        let outcome = parser.parse("minimize latency under 100ms");
        match outcome {
            ParseOutcome::Parsed(goal) => {
                assert_eq!(goal.goal_type, GoalType::Optimize);
                assert!(!goal.success_criteria.is_empty());
            }
            other => panic!("expected Parsed, got {:?}", other),
        }
    }

    #[test]
    fn parse_explore_goal_needs_clarification_when_vague() {
        let parser = GoalParser::new();
        let outcome = parser.parse("explore");
        assert!(matches!(outcome, ParseOutcome::NeedsClarification(_)));
    }

    #[test]
    fn parse_includes_milestone_when_date_present() {
        let parser = GoalParser::new();
        let outcome = parser.parse("reduce cost below $500 by 2025-06-01");
        match outcome {
            ParseOutcome::Parsed(goal) => {
                assert_eq!(goal.milestones.len(), 1);
            }
            other => panic!("expected Parsed, got {:?}", other),
        }
    }
}
