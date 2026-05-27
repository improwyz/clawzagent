use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// Optional identity modification suggested by the self-improvement loop.
/// Only IdentityState fields are eligible — IdentityCore modifications
/// are rejected at ProposalGatekeeper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityModification {
    /// Which IdentityState field to modify (e.g., "self_esteem", "behaviour").
    pub field: String,
    /// Numeric delta to apply to the field.
    pub delta: f32,
    /// Why this modification is being proposed.
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementProposal {
    pub proposal_id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub triggering_metrics: Vec<(clawz_core::metrics::PerfDimension, f32)>,
    pub suggested_changes: Vec<String>,
    pub confidence: f32,
    /// Optional identity state modification.
    pub identity_modification: Option<IdentityModification>,
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
    pub fn new(thresholds: HashMap<clawz_core::metrics::PerfDimension, f32>) -> Self {
        Self { thresholds }
    }
}

impl Evaluator for ThresholdEvaluator {
    fn evaluate(&self, metrics: &[(clawz_core::metrics::PerfDimension, f32)]) -> Vec<PerfGap> {
        metrics
            .iter()
            .filter_map(|(dim, current)| {
                self.thresholds.get(dim).and_then(|&target| {
                    if *current < target {
                        Some(PerfGap {
                            dimension: *dim,
                            current: *current,
                            target,
                        })
                    } else {
                        None
                    }
                })
            })
            .collect()
    }
}

pub struct SimplePatternRecognizer;

impl PatternRecognizer for SimplePatternRecognizer {
    fn recognize(&self, gaps: &[PerfGap]) -> Vec<Pattern> {
        if gaps.is_empty() {
            return vec![];
        }
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
        patterns
            .iter()
            .map(|p| ImprovementProposal {
                proposal_id: Uuid::new_v4(),
                generated_at: Utc::now(),
                triggering_metrics: p.gaps.iter().map(|g| (g.dimension, g.current)).collect(),
                suggested_changes: vec![format!("address {} gap(s): {}", p.gaps.len(), p.name)],
                confidence: 0.8,
                identity_modification: None,
            })
            .collect()
    }
}

/// Orchestrates the full closed self-improvement pipeline:
/// outcome tracking → metric evaluation → pattern recognition →
/// proposal generation → governance → application
pub struct SelfImprovementLoop {
    outcome_tracker: Arc<super::outcome_tracker::OutcomeTracker>,
    evaluator: Arc<ThresholdEvaluator>,
    recognizer: Arc<SimplePatternRecognizer>,
    generator: Arc<BasicImprovementGenerator>,
    gatekeeper: Arc<crate::governance::proposal_gate::ProposalGatekeeper>,
    adaptor: Arc<super::behavioral_adaptor::BehavioralAdaptor>,
}

impl SelfImprovementLoop {
    /// Create a new loop with all required components.
    pub fn new(
        outcome_tracker: Arc<super::outcome_tracker::OutcomeTracker>,
        evaluator: Arc<ThresholdEvaluator>,
        recognizer: Arc<SimplePatternRecognizer>,
        generator: Arc<BasicImprovementGenerator>,
        gatekeeper: Arc<crate::governance::proposal_gate::ProposalGatekeeper>,
        adaptor: Arc<super::behavioral_adaptor::BehavioralAdaptor>,
    ) -> Self {
        Self {
            outcome_tracker,
            evaluator,
            recognizer,
            generator,
            gatekeeper,
            adaptor,
        }
    }

    /// Run one iteration: metrics → gaps → patterns → proposals → gatekeeper → apply
    pub async fn run_once(
        &self,
    ) -> Result<Vec<super::behavioral_adaptor::AppliedChange>, clawz_core::ClawzError> {
        let metrics = self.outcome_tracker.current_metrics().await;
        let gaps = self.evaluator.evaluate(&metrics);
        if gaps.is_empty() {
            return Ok(vec![]);
        }

        let patterns = self.recognizer.recognize(&gaps);
        if patterns.is_empty() {
            return Ok(vec![]);
        }

        let proposals = self.generator.generate(&patterns);
        let mut all_changes = Vec::new();
        for proposal in proposals {
            let approved = self.gatekeeper.route(proposal.clone()).await?.is_approved();
            if approved {
                let mut proposal_to_apply = proposal.clone();
                // Wire identity_modification into suggested_changes so parse_and_apply can consume it.
                if let Some(ref identity_mod) = proposal.identity_modification {
                    let suggestion =
                        format!("identity {} {}", identity_mod.field, identity_mod.delta);
                    proposal_to_apply.suggested_changes.push(suggestion);
                }
                let changes = self.adaptor.apply(&proposal_to_apply).await?;
                all_changes.extend(changes);
            }
        }
        Ok(all_changes)
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

    #[tokio::test]
    async fn self_improvement_loop_runs_without_panic() {
        // Smoke test: SelfImprovementLoop constructs and run_once completes.
        // Uses empty thresholds so no gaps are detected → empty result.
        use crate::governance::skill_repository::SkillRepository;

        struct DummySkillRepo;
        #[async_trait::async_trait]
        impl SkillRepository for DummySkillRepo {
            async fn get_skill(
                &self,
                _: &str,
            ) -> Result<
                Option<crate::governance::skill_repository::SkillBundle>,
                clawz_core::ClawzError,
            > {
                Ok(None)
            }
            async fn update_skill(
                &self,
                _: &str,
                _: crate::governance::skill_repository::SkillBundle,
            ) -> Result<(), clawz_core::ClawzError> {
                Ok(())
            }
        }

        let tracker = Arc::new(crate::memory::outcome_tracker::OutcomeTracker::new(3));
        let evaluator = Arc::new(ThresholdEvaluator::new(HashMap::new()));
        let recognizer = Arc::new(SimplePatternRecognizer);
        let generator = Arc::new(BasicImprovementGenerator);
        let gatekeeper = Arc::new(crate::governance::proposal_gate::ProposalGatekeeper::new(
            crate::governance::proposal_gate::GateConfig {
                mode: clawz_core::deployment::DeploymentMode::Micro,
                required_approvals: 0,
            },
            Arc::new(crate::governance::approval::ApprovalWorkflow::new()),
            Arc::new(crate::governance::audit::AuditLogger::new()),
            None,
        ));
        let adaptor = Arc::new(crate::memory::behavioral_adaptor::BehavioralAdaptor::new(
            Arc::new(DummySkillRepo),
        ));

        let loop_ = SelfImprovementLoop::new(
            tracker, evaluator, recognizer, generator, gatekeeper, adaptor,
        );
        // With no thresholds, no gaps → empty result, no panic.
        let result = loop_.run_once().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn improvement_proposal_can_carry_identity_modification() {
        let proposal = ImprovementProposal {
            proposal_id: uuid::Uuid::new_v4(),
            generated_at: chrono::Utc::now(),
            triggering_metrics: vec![],
            suggested_changes: vec![],
            confidence: 0.8,
            identity_modification: Some(IdentityModification {
                field: "self_esteem".to_string(),
                delta: 0.1,
                reason: "positive feedback from recent successes".to_string(),
            }),
        };

        assert!(proposal.identity_modification.is_some());
        assert_eq!(
            proposal.identity_modification.as_ref().unwrap().field,
            "self_esteem"
        );
    }
}
