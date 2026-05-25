use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfGap {
    pub dimension: clawz_core::metrics::PerfDimension,
    pub current: f32,
    pub target: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub name: String,
    pub description: String,
    pub gaps: Vec<PerfGap>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementProposal {
    pub proposal_id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub triggering_metrics: Vec<(clawz_core::metrics::PerfDimension, f32)>,
    pub suggested_changes: Vec<String>,
    pub confidence: f32,
}

pub trait Evaluator: Send + Sync {
    fn evaluate(&self, metrics: &[(clawz_core::metrics::PerfDimension, f32)]) -> Vec<PerfGap>;
}

pub trait PatternRecognizer: Send + Sync {
    fn recognize(&self, gaps: &[PerfGap]) -> Vec<Pattern>;
}

pub trait ImprovementGenerator: Send + Sync {
    fn generate(&self, patterns: &[Pattern]) -> Vec<ImprovementProposal>;
}

pub struct ThresholdEvaluator {
    thresholds: HashMap<clawz_core::metrics::PerfDimension, f32>,
}

impl ThresholdEvaluator {
    pub fn new(thresholds: HashMap<clawz_core::metrics::PerfDimension, f32>) -> Self { Self { thresholds } }
}

impl Evaluator for ThresholdEvaluator {
    fn evaluate(&self, metrics: &[(clawz_core::metrics::PerfDimension, f32)]) -> Vec<PerfGap> {
        metrics.iter()
            .filter_map(|(dim, current)| {
                self.thresholds.get(dim).and_then(|&target| {
                    if *current < target {
                        Some(PerfGap { dimension: *dim, current: *current, target })
                    } else { None }
                })
            })
            .collect()
    }
}

pub struct SimplePatternRecognizer;

impl PatternRecognizer for SimplePatternRecognizer {
    fn recognize(&self, gaps: &[PerfGap]) -> Vec<Pattern> {
        if gaps.is_empty() { return vec![]; }
        vec![Pattern {
            name: "performance_gaps".into(),
            description: format!("{} dimensions below threshold", gaps.len()),
            gaps: gaps.to_vec(),
        }]
    }
}

pub struct BasicImprovementGenerator;

impl ImprovementGenerator for BasicImprovementGenerator {
    fn generate(&self, patterns: &[Pattern]) -> Vec<ImprovementProposal> {
        patterns.iter().map(|p| ImprovementProposal {
            proposal_id: Uuid::new_v4(),
            generated_at: Utc::now(),
            triggering_metrics: p.gaps.iter().map(|g| (g.dimension, g.current)).collect(),
            suggested_changes: vec![format!("address {} gap(s): {}", p.gaps.len(), p.name)],
            confidence: 0.8,
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_evaluator_detects_gap() {
        let mut thresholds = HashMap::new();
        thresholds.insert(clawz_core::metrics::PerfDimension::Speed, 0.9);
        thresholds.insert(clawz_core::metrics::PerfDimension::Accuracy, 0.85);
        let evaluator = ThresholdEvaluator::new(thresholds);
        let metrics = vec![
            (clawz_core::metrics::PerfDimension::Speed, 0.75),
            (clawz_core::metrics::PerfDimension::Accuracy, 0.90),
        ];
        let gaps = evaluator.evaluate(&metrics);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].dimension, clawz_core::metrics::PerfDimension::Speed);
    }

    #[test]
    fn basic_generator_produces_proposal() {
        let generator = BasicImprovementGenerator;
        let patterns = vec![Pattern {
            name: "speed_gap".into(),
            description: "Speed below threshold".into(),
            gaps: vec![PerfGap {
                dimension: clawz_core::metrics::PerfDimension::Speed,
                current: 0.75,
                target: 0.90,
            }],
        }];
        let proposals = generator.generate(&patterns);
        assert_eq!(proposals.len(), 1);
        assert!(!proposals[0].proposal_id.to_string().is_empty());
        assert_eq!(proposals[0].triggering_metrics.len(), 1);
        assert_eq!(proposals[0].confidence, 0.8);
    }
}
