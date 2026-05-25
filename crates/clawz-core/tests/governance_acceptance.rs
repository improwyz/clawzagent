//! PRISM-G Governance dimension acceptance tests.
//!
//! These tests verify the OversightLevel implementation and PRISM-G G dimension
//! governance contracts.

use clawz_core::types::governance::OversightLevel;

#[test]
fn oversight_level_autonomy_ranking() {
    // Autonomous (rank 4) > Monitored (rank 3)
    assert!(
        OversightLevel::Autonomous.autonomy_rank() > OversightLevel::Monitored.autonomy_rank(),
        "Autonomous should have higher autonomy rank than Monitored"
    );
    // Monitored (rank 3) > HumanInTheLoop (rank 2)
    assert!(
        OversightLevel::Monitored.autonomy_rank() > OversightLevel::HumanInTheLoop.autonomy_rank(),
        "Monitored should have higher autonomy rank than HumanInTheLoop"
    );
    // HumanInTheLoop (rank 2) > HumanDirected (rank 1)
    assert!(
        OversightLevel::HumanInTheLoop.autonomy_rank() > OversightLevel::HumanDirected.autonomy_rank(),
        "HumanInTheLoop should have higher autonomy rank than HumanDirected"
    );
    // HumanDirected (rank 1) > Manual (rank 0)
    assert!(
        OversightLevel::HumanDirected.autonomy_rank() > OversightLevel::Manual.autonomy_rank(),
        "HumanDirected should have higher autonomy rank than Manual"
    );
}

#[test]
fn requires_pre_approval_higher_levels() {
    // Autonomous and Monitored do NOT require human decision
    assert!(
        !OversightLevel::Autonomous.requires_human_decision(),
        "Autonomous should not require human decision"
    );
    assert!(
        !OversightLevel::Monitored.requires_human_decision(),
        "Monitored should not require human decision"
    );

    // HumanInTheLoop, HumanDirected, and Manual DO require human decision
    assert!(
        OversightLevel::HumanInTheLoop.requires_human_decision(),
        "HumanInTheLoop should require human decision"
    );
    assert!(
        OversightLevel::HumanDirected.requires_human_decision(),
        "HumanDirected should require human decision"
    );
    assert!(
        OversightLevel::Manual.requires_human_decision(),
        "Manual should require human decision"
    );
}

#[test]
fn oversight_all_five_levels() {
    assert_eq!(
        OversightLevel::ALL.len(),
        5,
        "OversightLevel should have exactly 5 levels"
    );

    // Verify all expected variants are present
    assert!(OversightLevel::ALL.contains(&OversightLevel::Autonomous));
    assert!(OversightLevel::ALL.contains(&OversightLevel::Monitored));
    assert!(OversightLevel::ALL.contains(&OversightLevel::HumanInTheLoop));
    assert!(OversightLevel::ALL.contains(&OversightLevel::HumanDirected));
    assert!(OversightLevel::ALL.contains(&OversightLevel::Manual));
}

#[test]
fn oversight_autonomy_rank_is_correct() {
    assert_eq!(OversightLevel::Autonomous.autonomy_rank(), 4);
    assert_eq!(OversightLevel::Monitored.autonomy_rank(), 3);
    assert_eq!(OversightLevel::HumanInTheLoop.autonomy_rank(), 2);
    assert_eq!(OversightLevel::HumanDirected.autonomy_rank(), 1);
    assert_eq!(OversightLevel::Manual.autonomy_rank(), 0);
}
