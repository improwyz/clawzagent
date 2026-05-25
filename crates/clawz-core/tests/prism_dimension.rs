use clawz_core::prism::{DimensionStatus, PrismDimension};

#[test]
fn all_six_dimensions_present_in_canonical_order() {
    let all = PrismDimension::ALL;
    assert_eq!(all.len(), 6);
    let letters: String = all.iter().map(|d| d.letter()).collect();
    assert_eq!(letters, "PRISMG");
    assert_eq!(all[0], PrismDimension::Purpose);
    assert_eq!(all[3], PrismDimension::Swarm);
    assert_eq!(all[5], PrismDimension::Governance);
}

#[test]
fn titles_match_canonical_names() {
    assert_eq!(PrismDimension::Memory.title(), "Memory & Metrics");
    assert_eq!(PrismDimension::Swarm.title(), "Swarm");
}

#[test]
fn serde_roundtrip_is_snake_case() {
    let json = serde_json::to_string(&PrismDimension::Infrastructure).unwrap();
    assert_eq!(json, "\"infrastructure\"");
    let back: PrismDimension = serde_json::from_str(&json).unwrap();
    assert_eq!(back, PrismDimension::Infrastructure);
}

#[test]
fn status_is_honest_three_state() {
    let s = DimensionStatus::Planned;
    assert_eq!(serde_json::to_string(&s).unwrap(), "\"planned\"");
}

#[test]
fn governance_is_the_only_implemented_dimension_at_phase_0() {
    use clawz_core::deployment::DeploymentMode;
    use clawz_core::prism::derive_status;

    let reg = clawz_core::prism::dimension_registry();
    let gov = reg
        .iter()
        .find(|i| i.dimension == PrismDimension::Governance)
        .unwrap();
    // At Phase 0, Governance is Implemented (the rename is done), all others Planned.
    assert_eq!(
        derive_status(
            gov,
            &[
                DeploymentMode::Standalone,
                DeploymentMode::Micro,
                DeploymentMode::Elastic
            ]
        ),
        DimensionStatus::Implemented
    );
    let purpose = reg
        .iter()
        .find(|i| i.dimension == PrismDimension::Purpose)
        .unwrap();
    assert_eq!(derive_status(purpose, &[]), DimensionStatus::Planned);
}
