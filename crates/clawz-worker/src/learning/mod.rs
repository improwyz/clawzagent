//! Shared learning stack: outcome tracking, self-improvement, skills, identity.
//!
//! Constructed once per [`WorkerService`](crate::service::WorkerService) and
//! attached to [`RuntimeDependencies`](crate::runtime::agent::RuntimeDependencies)
//! for normal agent turns (not cron/background).

use std::collections::HashMap;
use std::sync::Arc;

use clawz_core::deployment::DeploymentMode;
use clawz_core::error::Result;
use clawz_core::metrics::PerfDimension;

use crate::governance::approval::ApprovalWorkflow;
use crate::governance::audit::AuditLogger;
use crate::governance::proposal_gate::{GateConfig, ProposalGatekeeper};
use crate::governance::skill_repository::{SkillRepository, VersionedSkillRepository};
use crate::memory::improvement::{
    BasicImprovementGenerator, SelfImprovementLoop, SimplePatternRecognizer, ThresholdEvaluator,
};
use crate::memory::outcome_tracker::OutcomeTracker;
use crate::memory::post_turn_nudge::PostTurnMemoryNudge;
use crate::memory::transcript_search::TranscriptFtsIndex;
use crate::memory::user_profile::UserProfileStore;
use crate::runtime::identity::{AgentIdentityStore, FileIdentityBackend};

mod auditing_skill_repo;
use auditing_skill_repo::AuditingSkillRepository;

/// Shared learning components for agent runtimes.
pub struct LearningStack {
    outcome_tracker: Arc<OutcomeTracker>,
    identity_store: Arc<AgentIdentityStore>,
    skill_repository: Arc<dyn SkillRepository>,
    proposal_gatekeeper: Arc<ProposalGatekeeper>,
    self_improvement_loop: Arc<SelfImprovementLoop>,
    audit_logger: Arc<AuditLogger>,
    transcript_search: Arc<TranscriptFtsIndex>,
    user_profiles: Arc<UserProfileStore>,
    post_turn_nudge: Arc<PostTurnMemoryNudge>,
    interval_turns: usize,
    enabled: bool,
}

impl LearningStack {
    /// Build the stack from environment and deployment mode.
    pub async fn from_env(approval_workflow: Arc<ApprovalWorkflow>) -> Result<Self> {
        let enabled = std::env::var("CLAWZ_SELF_IMPROVEMENT")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);

        let interval_turns = std::env::var("CLAWZ_SELF_IMPROVEMENT_INTERVAL_TURNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(5);

        let failure_threshold = std::env::var("CLAWZ_OUTCOME_FAILURE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let outcome_tracker = Arc::new(OutcomeTracker::new(failure_threshold));
        let identity_store = Arc::new(AgentIdentityStore::with_backend(Arc::new(
            FileIdentityBackend::default_home(),
        )));
        let audit_logger = Arc::new(AuditLogger::new());
        let mode = DeploymentMode::from_env();
        let inner_skills: Arc<dyn SkillRepository> = match mode {
            DeploymentMode::Standalone => Arc::new(VersionedSkillRepository::in_memory()),
            DeploymentMode::Micro | DeploymentMode::Elastic => {
                let root = crate::workspace::WorkspaceLoader::default_home()
                    .root()
                    .to_path_buf();
                Arc::new(VersionedSkillRepository::from_enterprise_workspace(root))
            }
        };
        let skill_repository: Arc<dyn SkillRepository> = Arc::new(AuditingSkillRepository::new(
            inner_skills,
            audit_logger.clone(),
        ));

        let required_approvals = match mode {
            DeploymentMode::Standalone => 0,
            DeploymentMode::Micro => 2,
            DeploymentMode::Elastic => 1,
        };

        let proposal_gatekeeper = Arc::new(ProposalGatekeeper::new(
            GateConfig {
                mode,
                required_approvals,
            },
            approval_workflow,
            audit_logger.clone(),
            None,
        ));

        let adaptor = Arc::new(crate::memory::behavioral_adaptor::BehavioralAdaptor::new(
            skill_repository.clone(),
        ));

        let mut thresholds = HashMap::new();
        thresholds.insert(PerfDimension::Speed, 0.85);
        thresholds.insert(PerfDimension::Accuracy, 0.80);
        thresholds.insert(PerfDimension::Reliability, 0.85);

        let self_improvement_loop = Arc::new(SelfImprovementLoop::new(
            outcome_tracker.clone(),
            Arc::new(ThresholdEvaluator::new(thresholds)),
            Arc::new(SimplePatternRecognizer),
            Arc::new(BasicImprovementGenerator),
            proposal_gatekeeper.clone(),
            adaptor,
            audit_logger.clone(),
        ));

        let transcript_search = Arc::new(TranscriptFtsIndex::open_default().await?);
        let user_profiles = Arc::new(UserProfileStore::default_home());
        let post_turn_nudge = Arc::new(PostTurnMemoryNudge::from_env());

        Ok(Self {
            outcome_tracker,
            identity_store,
            skill_repository,
            proposal_gatekeeper,
            self_improvement_loop,
            audit_logger,
            transcript_search,
            user_profiles,
            post_turn_nudge,
            interval_turns,
            enabled,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn interval_turns(&self) -> usize {
        self.interval_turns
    }

    pub fn outcome_tracker(&self) -> Arc<OutcomeTracker> {
        self.outcome_tracker.clone()
    }

    pub fn identity_store(&self) -> Arc<AgentIdentityStore> {
        self.identity_store.clone()
    }

    pub fn skill_repository(&self) -> Arc<dyn SkillRepository> {
        self.skill_repository.clone()
    }

    pub fn proposal_gatekeeper(&self) -> Arc<ProposalGatekeeper> {
        self.proposal_gatekeeper.clone()
    }

    pub fn self_improvement_loop(&self) -> Arc<SelfImprovementLoop> {
        self.self_improvement_loop.clone()
    }

    pub fn audit_logger(&self) -> Arc<AuditLogger> {
        self.audit_logger.clone()
    }

    pub fn transcript_search(&self) -> Arc<TranscriptFtsIndex> {
        self.transcript_search.clone()
    }

    pub fn user_profiles(&self) -> Arc<UserProfileStore> {
        self.user_profiles.clone()
    }

    pub fn post_turn_nudge(&self) -> Arc<PostTurnMemoryNudge> {
        self.post_turn_nudge.clone()
    }
}
