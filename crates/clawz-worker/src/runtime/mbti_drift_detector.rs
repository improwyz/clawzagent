use crate::runtime::identity::AgentIdentity;
use crate::runtime::identity_types::{BehaviourType, MBTIType};

/// Detects when an agent's observed behaviour no longer matches its seeded MBTI type.
/// After `min_sessions` and with `confidence_threshold` match to another type,
/// proposes an MBTI drift label update.
#[derive(Debug, Clone)]
pub struct MBTIDriftDetector {
    pub min_sessions: u64,
    pub confidence_threshold: f32,
}

impl MBTIDriftDetector {
    pub fn new(min_sessions: u64, confidence_threshold: f32) -> Self {
        Self { min_sessions, confidence_threshold }
    }

    /// Returns `Some(new_label)` if drift is detected, `None` otherwise.
    pub fn detect_drift(&self, identity: &AgentIdentity) -> Option<MBTIType> {
        if identity.session_count < self.min_sessions {
            return None;
        }

        let inferred_type = self.infer_type_from_behaviour(&identity.state.behaviour)?;

        if inferred_type != identity.core.mbti.as_str()
            && self.compute_confidence(&identity.state.behaviour, inferred_type)
                >= self.confidence_threshold
        {
            Some(MBTIType::new(inferred_type))
        } else {
            None
        }
    }

    fn infer_type_from_behaviour(&self, behaviour: &std::collections::HashMap<BehaviourType, f32>) -> Option<&'static str> {
        use crate::runtime::identity_types::BehaviourType;
        let dominant = behaviour.iter()
            .filter(|(_, v)| **v > 0.6)
            .map(|(k, _)| *k)
            .collect::<Vec<_>>();

        if dominant.is_empty() {
            return None;
        }

        let has_assertive = dominant.contains(&BehaviourType::Assertive);
        let has_seeking = dominant.contains(&BehaviourType::Seeking);
        let has_cooperative = dominant.contains(&BehaviourType::Cooperative);

        Some(match (has_assertive, has_seeking, has_cooperative) {
            (true, true, false) => "ENTP",
            (true, false, true) => "ENTJ",
            (false, true, true) => "ENFP",
            _ => "INTJ",
        })
    }

    fn compute_confidence(&self, behaviour: &std::collections::HashMap<BehaviourType, f32>, _inferred_type: &str) -> f32 {
        let active_behaviours: f32 = behaviour.values()
            .filter(|&&v| v > 0.5)
            .sum();
        (active_behaviours / 3.0).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::AgentIdentity;
    use crate::runtime::identity_types::BehaviourType;

    fn create_test_identity(mbti_str: &str) -> AgentIdentity {
        let mut identity = AgentIdentity::new_for_testing("test-agent");
        identity.core.mbti = MBTIType::new(mbti_str);
        identity.core.original_mbti = MBTIType::new(mbti_str);
        identity
    }

    #[test]
    fn no_drift_below_min_sessions() {
        let detector = MBTIDriftDetector::new(50, 0.8);
        let mut identity = create_test_identity("INTJ");
        identity.session_count = 49;
        assert!(detector.detect_drift(&identity).is_none());
    }

    #[test]
    fn no_drift_below_confidence_threshold() {
        let detector = MBTIDriftDetector::new(3, 0.8);
        let mut identity = create_test_identity("INTJ");
        identity.session_count = 3;
        // low confidence - only 1 behaviour above threshold
        identity.state.behaviour.insert(BehaviourType::Cooperative, 0.7);
        assert!(detector.detect_drift(&identity).is_none());
    }

    #[test]
    fn drift_detected_above_threshold() {
        let detector = MBTIDriftDetector::new(3, 0.8);
        let mut identity = create_test_identity("INTJ");
        identity.session_count = 3;
        // high confidence - 3 behaviours above threshold, mapping to ENTP
        identity.state.behaviour.insert(BehaviourType::Assertive, 0.9);
        identity.state.behaviour.insert(BehaviourType::Seeking, 0.9);
        identity.state.behaviour.insert(BehaviourType::Impulsive, 0.8);
        let drift = detector.detect_drift(&identity);
        assert!(drift.is_some());
        // INTJ original preserved
        assert_eq!(identity.core.original_mbti.as_str(), "INTJ");
        // drift label is different
        assert_ne!(drift.unwrap().as_str(), "INTJ");
    }
}