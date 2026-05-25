use clawz_core::types::tool_risk::*;

#[test]
fn risk_level_approval_mode_mapping() {
    assert_eq!(RiskLevel::Low.approval(), ApprovalMode::Automatic);
    assert_eq!(RiskLevel::Medium.approval(), ApprovalMode::Logged);
    assert_eq!(RiskLevel::High.approval(), ApprovalMode::HumanRequired);
}

#[test]
fn action_primitive_serde_roundtrip() {
    let p = ActionPrimitive::Analyze;
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(json, "\"analyze\"");
    let back: ActionPrimitive = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
}

#[test]
fn backoff_serde_roundtrip() {
    let b = Backoff::Exponential {
        base_ms: 100,
        max_ms: 5000,
    };
    let json = serde_json::to_string(&b).unwrap();
    let back: Backoff = serde_json::from_str(&json).unwrap();
    assert_eq!(back, b);
}

#[test]
fn composition_serde_roundtrip() {
    let c = Composition::Parallel;
    let json = serde_json::to_string(&c).unwrap();
    assert_eq!(json, "\"parallel\"");
    let back: Composition = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
}

#[test]
fn all_eight_primitives_serialize() {
    let primitives = [
        ActionPrimitive::Read,
        ActionPrimitive::Write,
        ActionPrimitive::Transform,
        ActionPrimitive::Analyze,
        ActionPrimitive::Notify,
        ActionPrimitive::Execute,
        ActionPrimitive::Decide,
        ActionPrimitive::Wait,
    ];
    for p in &primitives {
        let json = serde_json::to_string(p).unwrap();
        let back: ActionPrimitive = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, p);
    }
}
