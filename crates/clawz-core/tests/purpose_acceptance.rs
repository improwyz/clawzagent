use clawz_core::types::{
    ClarificationQuestion, Comparator, Constraint, ConstraintKind, GoalObject, GoalType,
    Milestone, ParseOutcome, RewardWeights, SuccessCriterion,
};

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
        .success_criteria(vec![SuccessCriterion::new(
            "cost",
            Comparator::LessThan,
            serde_json::json!(5000.0),
        )])
        .milestones(vec![Milestone::new("q1-review").with_due_date(chrono::Utc::now())])
        .reward_weights(RewardWeights {
            accuracy: 0.1,
            speed: 0.2,
            cost: 0.6,
            safety: 0.1,
        })
        .build();

    let json = serde_json::to_string(&goal).unwrap();
    let back: GoalObject = serde_json::from_str(&json).unwrap();

    assert_eq!(back.description, "reduce cloud spend");
    assert_eq!(back.goal_type, GoalType::Optimize);
    assert_eq!(back.constraints.len(), 1);
    assert_eq!(back.constraints[0].kind, ConstraintKind::Budget);
    assert_eq!(back.milestones.len(), 1);
    assert!(back.reward_weights.is_normalized(0.001));
}

#[test]
fn parse_outcome_serde_is_externally_tagged() {
    let parsed = ParseOutcome::Parsed(
        GoalObject::builder()
            .description("test")
            .success_criteria(vec![SuccessCriterion::new(
                "x",
                Comparator::GreaterThan,
                serde_json::json!(1),
            )])
            .reward_weights(RewardWeights {
                accuracy: 1.0,
                speed: 0.0,
                cost: 0.0,
                safety: 0.0,
            })
            .build(),
    );
    let json = serde_json::to_string(&parsed).unwrap();
    assert!(json.contains("Parsed"));

    let back: ParseOutcome = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, ParseOutcome::Parsed(_)));
}

#[test]
fn clarification_question_serde_roundtrip() {
    let q = ClarificationQuestion::new("What is the budget?", "user asked to save money");
    let json = serde_json::to_string(&q).unwrap();
    let back: ClarificationQuestion = serde_json::from_str(&json).unwrap();
    assert_eq!(back.question, "What is the budget?");
    assert_eq!(back.context, "user asked to save money");
}

#[test]
fn goal_type_serde_is_snake_case() {
    assert_eq!(serde_json::to_string(&GoalType::Optimize).unwrap(), "\"optimize\"");
    assert_eq!(serde_json::to_string(&GoalType::Maintain).unwrap(), "\"maintain\"");
    let back: GoalType = serde_json::from_str("\"explore\"").unwrap();
    assert_eq!(back, GoalType::Explore);
}

#[test]
fn comparator_serde_is_snake_case() {
    assert_eq!(
        serde_json::to_string(&Comparator::GreaterThan).unwrap(),
        "\"greater_than\""
    );
    assert_eq!(
        serde_json::to_string(&Comparator::AtLeast).unwrap(),
        "\"at_least\""
    );
    let back: Comparator = serde_json::from_str("\"less_than\"").unwrap();
    assert_eq!(back, Comparator::LessThan);
}

#[test]
fn constraint_kind_serde_is_snake_case() {
    assert_eq!(
        serde_json::to_string(&ConstraintKind::Compliance).unwrap(),
        "\"compliance\""
    );
    let back: ConstraintKind = serde_json::from_str("\"deadline\"").unwrap();
    assert_eq!(back, ConstraintKind::Deadline);
}

#[test]
fn reward_weights_default_is_uniform() {
    let rw = RewardWeights::default();
    assert!(rw.is_normalized(0.001));
    assert_eq!(rw.accuracy, 0.25);
    assert_eq!(rw.speed, 0.25);
    assert_eq!(rw.cost, 0.25);
    assert_eq!(rw.safety, 0.25);
}
