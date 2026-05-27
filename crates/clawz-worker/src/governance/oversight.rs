//! PRISM-G Governance dimension — oversight level enforcement.
//!
//! Maps `RiskLevel` to minimum required `OversightLevel`, determines effective
//! oversight based on active goals, and checks whether human pre-approval is
//! required before an action proceeds.

use clawz_core::types::{OversightLevel, tool_risk::RiskLevel};

/// Maps a `RiskLevel` to the minimum required `OversightLevel`.
///
/// Higher autonomy = more permissive.
/// Low risk → Autonomous, Medium risk → Monitored, High risk → HumanInTheLoop.
pub fn minimum_oversight_for_risk(risk: RiskLevel) -> OversightLevel {
    match risk {
        RiskLevel::Low => OversightLevel::Autonomous,
        RiskLevel::Medium => OversightLevel::Monitored,
        RiskLevel::High => OversightLevel::HumanInTheLoop,
    }
}

/// Determines the effective oversight level for an action given:
/// - The action's risk level
/// - The current governance policy (from engine config)
/// - Whether an active goal is set (Alignment guardrail triggers elevated oversight)
pub fn effective_oversight(
    risk: RiskLevel,
    configured_oversight: OversightLevel,
    has_active_goal: bool,
) -> OversightLevel {
    let minimum = minimum_oversight_for_risk(risk);

    // If we have an active goal (Alignment guardrail active), elevate to at least Monitored
    let elevated = if has_active_goal {
        match minimum.autonomy_rank() {
            r if r < OversightLevel::Monitored.autonomy_rank() => OversightLevel::Monitored,
            r if r < OversightLevel::HumanInTheLoop.autonomy_rank() => {
                OversightLevel::HumanInTheLoop
            }
            _ => minimum,
        }
    } else {
        minimum
    };

    // Take the more restrictive of minimum required and configured
    if configured_oversight.autonomy_rank() < elevated.autonomy_rank() {
        configured_oversight
    } else {
        elevated
    }
}

/// Returns true if the given oversight level requires human pre-approval.
pub fn requires_pre_approval(level: OversightLevel) -> bool {
    matches!(
        level,
        OversightLevel::HumanInTheLoop | OversightLevel::HumanDirected | OversightLevel::Manual
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::tool_risk::RiskLevel;

    #[test]
    fn minimum_oversight_low_risk_is_autonomous() {
        assert_eq!(
            minimum_oversight_for_risk(RiskLevel::Low),
            OversightLevel::Autonomous
        );
    }

    #[test]
    fn minimum_oversight_medium_risk_is_monitored() {
        assert_eq!(
            minimum_oversight_for_risk(RiskLevel::Medium),
            OversightLevel::Monitored
        );
    }

    #[test]
    fn minimum_oversight_high_risk_is_hitl() {
        assert_eq!(
            minimum_oversight_for_risk(RiskLevel::High),
            OversightLevel::HumanInTheLoop
        );
    }

    #[test]
    fn effective_oversight_without_goal_uses_minimum() {
        // Low risk + no goal → Autonomous (minimum for Low)
        let level = effective_oversight(RiskLevel::Low, OversightLevel::Autonomous, false);
        assert_eq!(level, OversightLevel::Autonomous);

        // Medium risk + no goal → Monitored (minimum for Medium)
        let level = effective_oversight(RiskLevel::Medium, OversightLevel::Autonomous, false);
        assert_eq!(level, OversightLevel::Monitored);

        // High risk + no goal → HumanInTheLoop (minimum for High)
        let level = effective_oversight(RiskLevel::High, OversightLevel::Autonomous, false);
        assert_eq!(level, OversightLevel::HumanInTheLoop);
    }

    #[test]
    fn effective_oversight_with_goal_elevates() {
        // Low risk + Autonomous + goal → Autonomous already satisfies "at least Monitored"
        let level = effective_oversight(RiskLevel::Low, OversightLevel::Autonomous, true);
        assert_eq!(level, OversightLevel::Autonomous);

        // Medium risk + Monitored + goal → Monitored (rank 3, already >= Monitored rank 3)
        let level = effective_oversight(RiskLevel::Medium, OversightLevel::Monitored, true);
        assert_eq!(level, OversightLevel::Monitored);

        // Low risk + Manual (more restrictive) → Manual wins
        let level = effective_oversight(RiskLevel::Low, OversightLevel::Manual, true);
        assert_eq!(level, OversightLevel::Manual);
    }

    #[test]
    fn effective_oversight_configured_more_restrictive_wins() {
        // Configured Manual > minimum for Low → Manual
        let level = effective_oversight(RiskLevel::Low, OversightLevel::Manual, false);
        assert_eq!(level, OversightLevel::Manual);

        // Configured HumanDirected > minimum for Medium → HumanDirected
        let level = effective_oversight(RiskLevel::Medium, OversightLevel::HumanDirected, false);
        assert_eq!(level, OversightLevel::HumanDirected);
    }

    #[test]
    fn requires_pre_approval_for_hitl_and_above() {
        assert!(!requires_pre_approval(OversightLevel::Autonomous));
        assert!(!requires_pre_approval(OversightLevel::Monitored));
        assert!(requires_pre_approval(OversightLevel::HumanInTheLoop));
        assert!(requires_pre_approval(OversightLevel::HumanDirected));
        assert!(requires_pre_approval(OversightLevel::Manual));
    }
}
