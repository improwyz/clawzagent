//! Purpose dimension types for the PRISM-G framework.
//!
//! The Purpose dimension structures vague human goals into machine-readable
//! objectives. These types are consumed by the worker's `purpose` module to
//! classify intent, extract constraints, validate goal objects, and feed
//! parsed goals into the agent pipeline.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Goal classification ───────────────────────────────────────────────────────

/// High-level intent category for a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalType {
    /// Maximize or minimize a metric.
    Optimize,
    /// Meet a set of requirements.
    Satisfy,
    /// Discover or learn.
    Explore,
    /// Keep a system in a desired state.
    Maintain,
}

impl GoalType {
    /// All goal types.
    pub const ALL: [GoalType; 4] = [
        GoalType::Optimize,
        GoalType::Satisfy,
        GoalType::Explore,
        GoalType::Maintain,
    ];

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            GoalType::Optimize => "optimize",
            GoalType::Satisfy => "satisfy",
            GoalType::Explore => "explore",
            GoalType::Maintain => "maintain",
        }
    }
}

// ── Constraints ───────────────────────────────────────────────────────────────

/// Kind of constraint placed on a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// Monetary budget limit.
    Budget,
    /// Time-bound deadline.
    Deadline,
    /// Resource availability (CPU, memory, GPU, etc.).
    Resource,
    /// Quality threshold (accuracy, latency, etc.).
    Quality,
    /// Regulatory or policy compliance requirement.
    Compliance,
}

/// A single constraint on a goal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub description: String,
    /// Numeric or string value; interpretation depends on `kind`.
    pub value: serde_json::Value,
}

impl Constraint {
    pub fn new(
        kind: ConstraintKind,
        description: impl Into<String>,
        value: serde_json::Value,
    ) -> Self {
        Self {
            kind,
            description: description.into(),
            value,
        }
    }
}

// ── Reward weights ────────────────────────────────────────────────────────────

/// Relative importance of different optimization dimensions.
///
/// Weights are normalized to sum to 1.0 by the validator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewardWeights {
    pub accuracy: f64,
    pub speed: f64,
    pub cost: f64,
    pub safety: f64,
}

impl Default for RewardWeights {
    fn default() -> Self {
        Self {
            accuracy: 0.25,
            speed: 0.25,
            cost: 0.25,
            safety: 0.25,
        }
    }
}

impl RewardWeights {
    /// Return `true` if all weights are non-negative and sum to ~1.0.
    pub fn is_normalized(&self, epsilon: f64) -> bool {
        let sum = self.accuracy + self.speed + self.cost + self.safety;
        (sum - 1.0).abs() < epsilon
            && self.accuracy >= 0.0
            && self.speed >= 0.0
            && self.cost >= 0.0
            && self.safety >= 0.0
    }
}

// ── Success criteria ──────────────────────────────────────────────────────────

/// Comparator for evaluating a metric against a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparator {
    GreaterThan,
    LessThan,
    EqualTo,
    AtLeast,
    AtMost,
}

/// A single success criterion: `metric` must compare to `target_value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuccessCriterion {
    pub metric: String,
    pub comparator: Comparator,
    /// JSON-encoded target (number, string, boolean, etc.).
    pub target_value: serde_json::Value,
}

impl SuccessCriterion {
    pub fn new(
        metric: impl Into<String>,
        comparator: Comparator,
        target_value: serde_json::Value,
    ) -> Self {
        Self {
            metric: metric.into(),
            comparator,
            target_value,
        }
    }
}

// ── Milestone ─────────────────────────────────────────────────────────────────

/// An intermediate checkpoint on the way to a goal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Milestone {
    pub name: String,
    pub due_date: Option<DateTime<Utc>>,
    pub criteria: Vec<SuccessCriterion>,
}

impl Milestone {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            due_date: None,
            criteria: Vec::new(),
        }
    }

    pub fn with_due_date(mut self, due: DateTime<Utc>) -> Self {
        self.due_date = Some(due);
        self
    }

    pub fn with_criteria(mut self, criteria: Vec<SuccessCriterion>) -> Self {
        self.criteria = criteria;
        self
    }
}

// ── GoalObject ────────────────────────────────────────────────────────────────

/// A fully-structured, machine-readable goal.
///
/// `GoalObject` is the central type of the Purpose dimension. It is produced
/// by [`GoalParser::parse`](crate::purpose::GoalParser) and consumed by the
/// agent runtime for planning and execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalObject {
    pub id: Uuid,
    pub description: String,
    pub goal_type: GoalType,
    pub constraints: Vec<Constraint>,
    pub success_criteria: Vec<SuccessCriterion>,
    pub milestones: Vec<Milestone>,
    pub reward_weights: RewardWeights,
    pub created_at: DateTime<Utc>,
}

impl GoalObject {
    /// Start building a [`GoalObject`] with sensible defaults.
    ///
    /// Defaults:
    /// - `id` = random UUID v4
    /// - `goal_type` = [`GoalType::Satisfy`]
    /// - `reward_weights` = uniform 0.25
    /// - `created_at` = now (UTC)
    /// - all collections empty
    pub fn builder() -> GoalObjectBuilder {
        GoalObjectBuilder::default()
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Builder for [`GoalObject`].
#[derive(Debug, Clone)]
pub struct GoalObjectBuilder {
    id: Uuid,
    description: String,
    goal_type: GoalType,
    constraints: Vec<Constraint>,
    success_criteria: Vec<SuccessCriterion>,
    milestones: Vec<Milestone>,
    reward_weights: RewardWeights,
    created_at: DateTime<Utc>,
}

impl Default for GoalObjectBuilder {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            description: String::new(),
            goal_type: GoalType::Satisfy,
            constraints: Vec::new(),
            success_criteria: Vec::new(),
            milestones: Vec::new(),
            reward_weights: RewardWeights::default(),
            created_at: Utc::now(),
        }
    }
}

impl GoalObjectBuilder {
    pub fn id(mut self, id: Uuid) -> Self {
        self.id = id;
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn goal_type(mut self, goal_type: GoalType) -> Self {
        self.goal_type = goal_type;
        self
    }

    pub fn constraints(mut self, constraints: Vec<Constraint>) -> Self {
        self.constraints = constraints;
        self
    }

    pub fn success_criteria(mut self, criteria: Vec<SuccessCriterion>) -> Self {
        self.success_criteria = criteria;
        self
    }

    pub fn milestones(mut self, milestones: Vec<Milestone>) -> Self {
        self.milestones = milestones;
        self
    }

    pub fn reward_weights(mut self, weights: RewardWeights) -> Self {
        self.reward_weights = weights;
        self
    }

    pub fn created_at(mut self, at: DateTime<Utc>) -> Self {
        self.created_at = at;
        self
    }

    pub fn build(self) -> GoalObject {
        GoalObject {
            id: self.id,
            description: self.description,
            goal_type: self.goal_type,
            constraints: self.constraints,
            success_criteria: self.success_criteria,
            milestones: self.milestones,
            reward_weights: self.reward_weights,
            created_at: self.created_at,
        }
    }
}

// ── Clarification ─────────────────────────────────────────────────────────────

/// A question the system asks when a goal is ambiguous or under-specified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClarificationQuestion {
    pub question: String,
    pub context: String,
}

impl ClarificationQuestion {
    pub fn new(question: impl Into<String>, context: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            context: context.into(),
        }
    }
}

// ── ParseOutcome ──────────────────────────────────────────────────────────────

/// Result of parsing a natural-language goal string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParseOutcome {
    /// Parsing succeeded and produced a structured goal.
    Parsed(GoalObject),
    /// The input was too vague; clarifying questions are returned.
    NeedsClarification(Vec<ClarificationQuestion>),
    /// Parsing failed with a human-readable reason.
    Failed(String),
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_type_all_four_variants() {
        assert_eq!(GoalType::ALL.len(), 4);
        assert!(GoalType::ALL.contains(&GoalType::Optimize));
        assert!(GoalType::ALL.contains(&GoalType::Explore));
    }

    #[test]
    fn reward_weights_default_is_normalized() {
        let rw = RewardWeights::default();
        assert!(rw.is_normalized(0.001));
        assert_eq!(rw.accuracy, 0.25);
    }

    #[test]
    fn reward_weights_rejects_negative() {
        let rw = RewardWeights {
            accuracy: -0.1,
            speed: 0.5,
            cost: 0.3,
            safety: 0.3,
        };
        assert!(!rw.is_normalized(0.001));
    }

    #[test]
    fn goal_object_builder_roundtrip() {
        let goal = GoalObject::builder()
            .description("reduce cloud spend")
            .goal_type(GoalType::Optimize)
            .constraints(vec![Constraint::new(
                ConstraintKind::Budget,
                "monthly budget",
                serde_json::json!(5000.0),
            )])
            .build();

        let json = serde_json::to_string(&goal).unwrap();
        let back: GoalObject = serde_json::from_str(&json).unwrap();

        assert_eq!(back.description, "reduce cloud spend");
        assert_eq!(back.goal_type, GoalType::Optimize);
        assert_eq!(back.constraints.len(), 1);
        assert_eq!(back.constraints[0].kind, ConstraintKind::Budget);
    }

    #[test]
    fn parse_outcome_serde_roundtrip() {
        let outcome = ParseOutcome::NeedsClarification(vec![ClarificationQuestion::new(
            "What is the budget?",
            "user asked to save money",
        )]);
        let json = serde_json::to_string(&outcome).unwrap();
        let back: ParseOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, outcome);
    }

    #[test]
    fn milestone_builder_chaining() {
        let m = Milestone::new("phase-1")
            .with_due_date(Utc::now())
            .with_criteria(vec![SuccessCriterion::new(
                "latency",
                Comparator::LessThan,
                serde_json::json!(100),
            )]);
        assert_eq!(m.name, "phase-1");
        assert_eq!(m.criteria.len(), 1);
    }
}
