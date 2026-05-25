//! Classify natural-language goal strings into [`GoalType`] variants.
//!
//! The heuristic is intentionally simple: keyword matching against the
//! input string. More sophisticated classification can be layered in later.

use clawz_core::types::GoalType;

/// Classify a free-form goal string into a [`GoalType`].
///
/// # Algorithm
/// 1. Lower-case the input.
/// 2. Scan for keywords associated with each goal type.
/// 3. Return the first match; default to [`GoalType::Satisfy`] if none found.
///
/// # Examples
/// ```
/// use clawz_core::types::GoalType;
/// use clawz_worker::purpose::classify;
///
/// assert_eq!(classify("minimize latency"), GoalType::Optimize);
/// assert_eq!(classify("explore new markets"), GoalType::Explore);
/// assert_eq!(classify("keep the system stable"), GoalType::Maintain);
/// assert_eq!(classify("fix the bug"), GoalType::Satisfy);
/// ```
pub fn classify(input: &str) -> GoalType {
    let lower = input.to_lowercase();

    let optimize_keywords = ["minimize", "maximize", "optimize", "reduce", "increase", "best"];
    let explore_keywords = ["explore", "discover", "learn", "investigate", "research"];
    let maintain_keywords = ["maintain", "keep", "preserve", "sustain", "stable"];

    if optimize_keywords.iter().any(|k| lower.contains(k)) {
        return GoalType::Optimize;
    }
    if explore_keywords.iter().any(|k| lower.contains(k)) {
        return GoalType::Explore;
    }
    if maintain_keywords.iter().any(|k| lower.contains(k)) {
        return GoalType::Maintain;
    }

    GoalType::Satisfy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_optimize_keywords() {
        assert_eq!(classify("minimize cost"), GoalType::Optimize);
        assert_eq!(classify("MAXIMIZE revenue"), GoalType::Optimize);
        assert_eq!(classify("optimize throughput"), GoalType::Optimize);
        assert_eq!(classify("reduce latency"), GoalType::Optimize);
        assert_eq!(classify("increase conversion"), GoalType::Optimize);
        assert_eq!(classify("find the best route"), GoalType::Optimize);
    }

    #[test]
    fn classify_explore_keywords() {
        assert_eq!(classify("explore options"), GoalType::Explore);
        assert_eq!(classify("DISCOVER anomalies"), GoalType::Explore);
        assert_eq!(classify("learn user preferences"), GoalType::Explore);
    }

    #[test]
    fn classify_maintain_keywords() {
        assert_eq!(classify("maintain uptime"), GoalType::Maintain);
        assert_eq!(classify("keep the database stable"), GoalType::Maintain);
        assert_eq!(classify("preserve state"), GoalType::Maintain);
    }

    #[test]
    fn classify_defaults_to_satisfy() {
        assert_eq!(classify("fix the login bug"), GoalType::Satisfy);
        assert_eq!(classify("deploy the service"), GoalType::Satisfy);
        assert_eq!(classify("write a poem"), GoalType::Satisfy);
    }
}
