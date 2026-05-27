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
            && self.compute_confidence(&identity.state.behaviour, Some(inferred_type))
                >= self.confidence_threshold
        {
            Some(MBTIType::new(inferred_type))
        } else {
            None
        }
    }

    fn infer_type_from_behaviour(&self, behaviour: &std::collections::HashMap<BehaviourType, f32>) -> Option<&'static str> {
        let dominant = behaviour.iter()
            .filter(|(_, v)| **v > 0.6)
            .map(|(k, _)| *k)
            .collect::<Vec<_>>();

        if dominant.is_empty() {
            return None;
        }

        let has_assertive = dominant.contains(&BehaviourType::Assertive);
        let has_aggressive = dominant.contains(&BehaviourType::Aggressive);
        let has_seeking = dominant.contains(&BehaviourType::Seeking);
        let has_impulsive = dominant.contains(&BehaviourType::Impulsive);
        let has_cooperative = dominant.contains(&BehaviourType::Cooperative);
        let has_compliant = dominant.contains(&BehaviourType::Compliant);
        let has_passive = dominant.contains(&BehaviourType::Passive);
        let has_ritualistic = dominant.contains(&BehaviourType::Ritualistic);

        // Map behaviours to MBTI dimensions
        // E/I dimension
        let is_extraverted = has_assertive || has_aggressive || has_seeking || has_impulsive;
        let is_introverted = has_passive || has_compliant || has_ritualistic;

        // S/N dimension
        let is_intuitive = has_seeking || has_impulsive;
        let is_sensing = has_compliant || has_ritualistic;

        // T/F dimension
        let is_thinking = has_assertive || has_aggressive;
        let is_feeling = has_cooperative || has_compliant;

        // J/P dimension
        let is_judging = has_aggressive || has_compliant || has_ritualistic;
        let is_perceiving = has_seeking || has_impulsive;

        // Build the type from resolved dimensions, or return None if ambiguous
        let e_or_i = if is_extraverted && !is_introverted {
            Some('E')
        } else if is_introverted && !is_extraverted {
            Some('I')
        } else {
            None
        };

        let s_or_n = if is_intuitive && !is_sensing {
            Some('N')
        } else if is_sensing && !is_intuitive {
            Some('S')
        } else {
            None
        };

        let t_or_f = if is_thinking && !is_feeling {
            Some('T')
        } else if is_feeling && !is_thinking {
            Some('F')
        } else {
            None
        };

        let j_or_p = if is_judging && !is_perceiving {
            Some('J')
        } else if is_perceiving && !is_judging {
            Some('P')
        } else {
            None
        };

        if let (Some(ei), Some(sn), Some(tf), Some(jp)) = (e_or_i, s_or_n, t_or_f, j_or_p) {
            Some(Box::leak(format!("{}{}{}{}", ei, sn, tf, jp).into_boxed_str()))
        } else {
            None
        }
    }

    fn compute_confidence(&self, behaviour: &std::collections::HashMap<BehaviourType, f32>, inferred_type: Option<&str>) -> f32 {
        let inferred = match inferred_type {
            Some(t) => t,
            None => return 0.0,
        };

        // Count how many of the inferred type's behaviours are actually present above threshold
        let dimension_chars: Vec<char> = inferred.chars().collect();
        let mut match_count: f32 = 0.0;

        for (btype, &value) in behaviour.iter() {
            if value < 0.5 {
                continue;
            }
            // Check if this behaviour type contributes to any of the inferred dimensions
            let contributes = match btype {
                BehaviourType::Assertive => dimension_chars.contains(&'E') || dimension_chars.contains(&'T'),
                BehaviourType::Aggressive => dimension_chars.contains(&'E') || dimension_chars.contains(&'T') || dimension_chars.contains(&'J'),
                BehaviourType::Seeking => dimension_chars.contains(&'E') || dimension_chars.contains(&'N') || dimension_chars.contains(&'P'),
                BehaviourType::Impulsive => dimension_chars.contains(&'E') || dimension_chars.contains(&'N') || dimension_chars.contains(&'P'),
                BehaviourType::Cooperative => dimension_chars.contains(&'E') || dimension_chars.contains(&'F'),
                BehaviourType::Compliant => dimension_chars.contains(&'I') || dimension_chars.contains(&'S') || dimension_chars.contains(&'F') || dimension_chars.contains(&'J'),
                BehaviourType::Passive => dimension_chars.contains(&'I'),
                BehaviourType::Ritualistic => dimension_chars.contains(&'I') || dimension_chars.contains(&'S') || dimension_chars.contains(&'J'),
            };
            if contributes {
                match_count += 1.0;
            }
        }

        // Normalize by total possible contributors (roughly 4 per dimension = 16, cap at 8)
        let max_relevant = if match_count < 1.0 { 1.0_f32 } else { 8.0_f32.min(match_count) };
        let confidence = match_count / max_relevant;
        confidence.clamp(0.0, 1.0)
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