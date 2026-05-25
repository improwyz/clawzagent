use clawz_core::metrics::PerfDimension;

#[test]
fn perf_dimension_serde_roundtrip() {
    let d = PerfDimension::GoalAlignment;
    let json = serde_json::to_string(&d).unwrap();
    assert_eq!(json, "\"goal_alignment\"");
    let back: PerfDimension = serde_json::from_str(&json).unwrap();
    assert_eq!(back, d);
}

#[test]
fn perf_dimension_all_six() {
    assert_eq!(PerfDimension::ALL.len(), 6);
}

#[test]
fn perf_dimension_title() {
    assert_eq!(PerfDimension::Accuracy.title(), "Accuracy");
    assert_eq!(PerfDimension::GoalAlignment.title(), "Goal Alignment");
}
