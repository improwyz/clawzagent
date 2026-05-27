use serde::{Deserialize, Serialize};

/// A full MBTI type — four letters, e.g. "INTJ" or "ENFP".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MBTIType(pub String);

impl MBTIType {
    pub fn new(s: &str) -> Self {
        Self(s.to_uppercase())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Temperament — reactivity and self-regulation baselines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Temperament {
    pub reactivity: f32,
    pub self_regulation: f32,
}

impl Default for Temperament {
    fn default() -> Self {
        Self {
            reactivity: 0.5,
            self_regulation: 0.5,
        }
    }
}

/// Risk posture — fixed threshold for acceptable risk.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RiskPosture {
    pub risk_tolerance: f32,
}

impl Default for RiskPosture {
    fn default() -> Self {
        Self {
            risk_tolerance: 0.5,
        }
    }
}

/// Processing style — fundamental cognitive mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProcessingStyle {
    Parallel,
    Sequential,
    Reflexive,
    #[default]
    Deliberative,
}

/// Authority orientation — core stance toward human oversight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthorityOrientation {
    Deferential,
    Skeptical,
    #[default]
    Egalitarian,
}

/// The agent's four immutable constitution principles.
pub const CARDINAL_RULES: &[&str] = &[
    "Be broadly safe — avoiding harm and respecting human oversight",
    "Be broadly ethical — honest, fair, and respectful of human rights-inspired norms",
    "Comply with Organization's guidelines and policies",
    "Be genuinely helpful to users — including long-term well-being rather than short-term desires",
];

/// Values — cardinal rules (immutable) and value hierarchy (evolvable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Values {
    pub cardinal_rules: Vec<String>,
    pub value_hierarchy: std::collections::HashMap<String, f32>,
}

impl Default for Values {
    fn default() -> Self {
        Self {
            cardinal_rules: CARDINAL_RULES.iter().map(|s| s.to_string()).collect(),
            value_hierarchy: std::collections::HashMap::new(),
        }
    }
}

/// Behaviour type tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BehaviourType {
    Cooperative,
    Assertive,
    Passive,
    Aggressive,
    Impulsive,
    Compliant,
    Ritualistic,
    Seeking,
}

/// Behaviour map (placeholder for future identity-state integration).
#[allow(dead_code)]
pub type BehaviourMap = std::collections::HashMap<BehaviourType, f32>;

/// Response calibration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseCalibration {
    pub directness: f32,
    pub assertiveness: f32,
    pub emotional_colour: f32,
}

impl Default for ResponseCalibration {
    fn default() -> Self {
        Self {
            directness: 0.5,
            assertiveness: 0.5,
            emotional_colour: 0.3,
        }
    }
}

/// Horizon baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Horizon {
    Short,
    #[default]
    Medium,
    Long,
}

/// Temporal preference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalPreference {
    pub horizon_baseline: Horizon,
    pub horizon_refinement: f32,
}

impl Default for TemporalPreference {
    fn default() -> Self {
        Self {
            horizon_baseline: Horizon::default(),
            horizon_refinement: 0.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mbti_type_new() {
        let mbti = MBTIType::new("entj");
        assert_eq!(mbti.as_str(), "ENTJ");
    }

    #[test]
    fn test_cardinal_rules_count() {
        assert_eq!(CARDINAL_RULES.len(), 4);
    }

    #[test]
    fn test_temperament_default() {
        let t = Temperament::default();
        assert_eq!(t.reactivity, 0.5);
    }

    #[test]
    fn test_values_default() {
        let v = Values::default();
        assert_eq!(v.cardinal_rules.len(), 4);
        assert!(v.value_hierarchy.is_empty());
    }
}
