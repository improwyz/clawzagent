use clawz_core::types::{ContextBundle, ContextDelta, DriftKind, DriftSignal, Reliability};

#[test]
fn context_bundle_builder_roundtrip() {
    let bundle = ContextBundle::builder()
        .tenant_id("acme")
        .version(7)
        .confidence(0.95)
        .build();

    let json = serde_json::to_string(&bundle).unwrap();
    let back: ContextBundle = serde_json::from_str(&json).unwrap();

    assert_eq!(back.bundle_id, bundle.bundle_id);
    assert_eq!(back.tenant_id, "acme");
    assert_eq!(back.version, 7);
    assert!((back.confidence - 0.95).abs() < f32::EPSILON);
}

#[test]
fn drift_signal_serde_is_snake_case() {
    let signal = DriftSignal {
        kind: DriftKind::PredictionError,
        detail: "cpu spiked".into(),
    };
    let json = serde_json::to_string(&signal).unwrap();
    assert!(json.contains("prediction_error"));

    let back: DriftSignal = serde_json::from_str(&json).unwrap();
    assert_eq!(back.kind, DriftKind::PredictionError);
}

#[test]
fn context_delta_serde() {
    let delta = ContextDelta {
        from_version: 3,
        to_version: 4,
        changes: vec!["limits updated".into()],
    };

    let json = serde_json::to_string(&delta).unwrap();
    let back: ContextDelta = serde_json::from_str(&json).unwrap();

    assert_eq!(back.from_version, 3);
    assert_eq!(back.to_version, 4);
    assert_eq!(back.changes, vec!["limits updated"]);
}

#[test]
fn reliability_enum_serde() {
    assert_eq!(
        serde_json::to_string(&Reliability::High).unwrap(),
        "\"high\""
    );
    assert_eq!(
        serde_json::to_string(&Reliability::Medium).unwrap(),
        "\"medium\""
    );
    assert_eq!(serde_json::to_string(&Reliability::Low).unwrap(), "\"low\"");

    let back: Reliability = serde_json::from_str("\"low\"").unwrap();
    assert_eq!(back, Reliability::Low);
}
