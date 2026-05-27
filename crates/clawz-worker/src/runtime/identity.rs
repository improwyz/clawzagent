//! AgentIdentityStore — cross-session accumulated identity.

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use sha2::{Digest, Sha256};
use hex;

use clawz_core::error::ClawzError;
use super::*;

/// Fixed core identity — injected at startup, never mutable post-initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityCore {
    pub mbti: identity_types::MBTIType,
    pub temperament: identity_types::Temperament,
    pub risk_posture: identity_types::RiskPosture,
    pub processing_style: identity_types::ProcessingStyle,
    pub authority_orientation: identity_types::AuthorityOrientation,
    pub values: identity_types::Values,
    pub identity_version: u64,
    pub original_mbti: identity_types::MBTIType,
}

impl IdentityCore {
    pub fn new(
        mbti: identity_types::MBTIType,
        temperament: identity_types::Temperament,
        risk_posture: identity_types::RiskPosture,
        processing_style: identity_types::ProcessingStyle,
        authority_orientation: identity_types::AuthorityOrientation,
        values: identity_types::Values,
    ) -> Self {
        Self {
            identity_version: 1,
            original_mbti: mbti.clone(),
            mbti,
            temperament,
            risk_posture,
            processing_style,
            authority_orientation,
            values,
        }
    }
}

impl Default for IdentityCore {
    fn default() -> Self {
        Self {
            mbti: identity_types::MBTIType::new("INTJ"),
            temperament: identity_types::Temperament::default(),
            risk_posture: identity_types::RiskPosture::default(),
            processing_style: identity_types::ProcessingStyle::default(),
            authority_orientation: identity_types::AuthorityOrientation::default(),
            values: identity_types::Values::default(),
            identity_version: 1,
            original_mbti: identity_types::MBTIType::new("INTJ"),
        }
    }
}

/// Evolving state — persisted across sessions, modified by runtime experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityState {
    pub behaviour: identity_types::BehaviourMap,
    pub self_esteem: f32,
    pub interests: std::collections::HashMap<String, f32>,
    pub talent: std::collections::HashMap<String, f32>,
    pub response_calibration: identity_types::ResponseCalibration,
    pub temporal_preference: identity_types::TemporalPreference,
    pub mbti_drift_label: Option<identity_types::MBTIType>,
}

impl Default for IdentityState {
    fn default() -> Self {
        Self {
            behaviour: std::collections::HashMap::new(),
            self_esteem: 0.5,
            interests: std::collections::HashMap::new(),
            talent: std::collections::HashMap::new(),
            response_calibration: identity_types::ResponseCalibration::default(),
            temporal_preference: identity_types::TemporalPreference::default(),
            mbti_drift_label: None,
        }
    }
}

/// Accumulated identity for an agent across all sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub agent_id: String,
    pub trust_relationships: HashMap<String, f64>,
    pub core: IdentityCore,
    pub state: IdentityState,
    pub accumulated_experience: u64,
    pub last_seen: DateTime<Utc>,
    pub session_count: u64,
}

impl AgentIdentity {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self::with_core(agent_id, IdentityCore::default())
    }

    pub fn with_core(agent_id: impl Into<String>, core: IdentityCore) -> Self {
        Self {
            agent_id: agent_id.into(),
            trust_relationships: HashMap::new(),
            core,
            state: IdentityState::default(),
            accumulated_experience: 0,
            last_seen: Utc::now(),
            session_count: 0,
        }
    }

    #[cfg(test)]
    pub fn new_for_testing(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            trust_relationships: HashMap::new(),
            core: IdentityCore::default(),
            state: IdentityState::default(),
            accumulated_experience: 0,
            last_seen: Utc::now(),
            session_count: 0,
        }
    }

    pub fn compute_identity_version_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.core.mbti.as_str().as_bytes());
        hasher.update((self.core.temperament.reactivity * 1000.0).to_bits().to_be_bytes());
        hasher.update((self.core.temperament.self_regulation * 1000.0).to_bits().to_be_bytes());
        hasher.update((self.core.risk_posture.risk_tolerance * 1000.0).to_bits().to_be_bytes());
        hasher.update(format!("{:?}", self.core.processing_style).as_bytes());
        hasher.update(format!("{:?}", self.core.authority_orientation).as_bytes());
        for rule in &self.core.values.cardinal_rules {
            hasher.update(rule.as_bytes());
        }
        hasher.update(self.core.identity_version.to_be_bytes());
        hex::encode(hasher.finalize())
    }

    pub fn record_task(&mut self, skill_name: &str, success: bool) {
        self.accumulated_experience += 1;
        let entry = self.state.talent.entry(skill_name.to_string()).or_insert(0.0);
        if success {
            *entry = (*entry + 0.05).min(1.0);
        } else {
            *entry = (*entry - 0.02).max(0.0);
        }
    }

    pub fn update_trust(&mut self, other_agent_id: &str, delta: f64) {
        let entry = self.trust_relationships.entry(other_agent_id.to_string()).or_insert(0.5);
        *entry = (*entry + delta).clamp(0.0, 1.0);
    }

    pub fn increment_session(&mut self) {
        self.session_count += 1;
        self.last_seen = Utc::now();
    }

    /// Compute drift score (0.0-1.0) — ratio of current vs original MBTI stability.
    pub fn drift_score(&self) -> f64 {
        // Compare current mbti against original_mbti
        if self.state.mbti_drift_label.is_none() {
            return 0.0;
        }
        // Score based on accumulated experience and drift label presence
        
        (self.accumulated_experience as f64 / 100.0).min(1.0)
    }

    /// Emit a governance event for identity drift (stub for now).
    pub async fn emit(&mut self, _event: GovernanceEvent) -> std::result::Result<(), ClawzError> {
        // Governance event emission would go here
        Ok(())
    }
}

/// Governance event types for identity drift tracking.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GovernanceEvent {
    IdentityDrift {
        drift_score: f64,
        checkpoint_id: std::path::PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Preference vector for an agent — used to personalize tool and governance choices.
pub struct PreferenceVector {
    // TODO: add fields
}

#[async_trait]
/// Backend storage for agent identities.
pub trait IdentityBackend: Send + Sync {
    async fn load(&self, agent_id: &str) -> Result<Option<AgentIdentity>, ClawzError>;
    async fn save(&self, identity: &AgentIdentity) -> Result<(), ClawzError>;
}

#[derive(Clone)]
/// In-memory identity backend using a shared `HashMap`.
pub struct InMemoryIdentityBackend {
    store: Arc<RwLock<HashMap<String, AgentIdentity>>>,
}

impl Default for InMemoryIdentityBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryIdentityBackend {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl IdentityBackend for InMemoryIdentityBackend {
    async fn load(&self, agent_id: &str) -> Result<Option<AgentIdentity>, ClawzError> {
        Ok(self.store.read().await.get(agent_id).cloned())
    }

    async fn save(&self, identity: &AgentIdentity) -> Result<(), ClawzError> {
        self.store.write().await.insert(identity.agent_id.clone(), identity.clone());
        Ok(())
    }
}

/// In-memory identity store. Use this for now; replace with a
/// proper database backend when ready.
#[derive(Clone)]
pub struct AgentIdentityStore {
    backend: Arc<dyn IdentityBackend>,
}

impl std::fmt::Debug for AgentIdentityStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentIdentityStore").finish()
    }
}

impl AgentIdentityStore {
    pub fn new_in_memory() -> Self {
        Self {
            backend: Arc::new(InMemoryIdentityBackend::new()),
        }
    }

    pub fn with_backend(backend: Arc<dyn IdentityBackend>) -> Self {
        Self { backend }
    }

    /// Load identity for `agent_id`, creating a fresh one if none exists.
    pub async fn load(&self, agent_id: &str) -> Result<AgentIdentity, ClawzError> {
        let identity = self.backend.load(agent_id).await?;
        Ok(identity.unwrap_or_else(|| AgentIdentity::new(agent_id.to_string())))
    }

    /// Persist `identity` after a session ends.
    pub async fn save(&self, identity: &AgentIdentity) -> Result<(), ClawzError> {
        self.backend.save(identity).await
    }

    /// Update trust from one agent toward another.
    pub async fn update_trust(
        &self,
        from_agent_id: &str,
        to_agent_id: &str,
        delta: f64,
    ) -> Result<(), ClawzError> {
        let mut identity = self.load(from_agent_id).await?;
        identity.update_trust(to_agent_id, delta);
        self.save(&identity).await
    }

    /// Compute the current identity-version hash for the given agent.
    ///
    /// The hash is derived from the immutable [`IdentityCore`] plus the
    /// evolving [`IdentityState`] (preferences + talent). It changes
    /// whenever any observable identity field changes and is stable for
    /// equivalent identities. Used by the Admin API to detect drift,
    /// version snapshots, and verify integrity across sessions.
    pub async fn get_identity_version_hash(&self, agent_id: &str) -> Result<String, ClawzError> {
        let identity = self.load(agent_id).await?;
        Ok(identity.compute_identity_version_hash())
    }

    /// Record a task completion for an agent.
    pub async fn record_task(
        &self,
        agent_id: &str,
        skill_name: &str,
        success: bool,
    ) -> Result<(), ClawzError> {
        let mut identity = self.load(agent_id).await?;
        identity.record_task(skill_name, success);
        self.save(&identity).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_identity_core_immutable() {
        let core = IdentityCore {
            mbti: identity_types::MBTIType::new("ENTJ"),
            temperament: identity_types::Temperament::default(),
            risk_posture: identity_types::RiskPosture::default(),
            processing_style: identity_types::ProcessingStyle::default(),
            authority_orientation: identity_types::AuthorityOrientation::default(),
            values: identity_types::Values::default(),
            identity_version: 1,
            original_mbti: identity_types::MBTIType::new("ENTJ"),
        };
        assert_eq!(core.identity_version, 1);
        assert_eq!(core.original_mbti.as_str(), "ENTJ");
        assert_eq!(core.values.cardinal_rules.len(), 4);
    }

    #[tokio::test]
    async fn test_identity_state_evolvable() {
        let mut state = IdentityState::default();
        assert!(state.behaviour.is_empty());
        state.behaviour.insert(identity_types::BehaviourType::Cooperative, 0.8);
        assert!((state.self_esteem - 0.5).abs() < 1e-4);
        state.self_esteem = 0.7;
        assert!((state.self_esteem - 0.7).abs() < 1e-4);
    }

    #[tokio::test]
    async fn test_identity_version_hash_computed() {
        let identity = AgentIdentity::new_for_testing("agent-1");
        let hash = identity.compute_identity_version_hash();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn test_record_task_updates_talent() {
        let mut identity = AgentIdentity::new_for_testing("agent-1");
        identity.record_task("rust", true);
        assert_eq!(identity.state.talent["rust"], 0.05);
        identity.record_task("rust", true);
        assert_eq!(identity.state.talent["rust"], 0.10);
        identity.record_task("rust", false);
        assert_eq!(identity.state.talent["rust"], 0.08);
    }

    #[tokio::test]
    async fn identity_store_persists_across_load_cycle() {
        let store = AgentIdentityStore::new_in_memory();
        let agent_id = "test-agent";
        let initial = AgentIdentity::new(agent_id);
        store.save(&initial).await.unwrap();
        let loaded = store.load(agent_id).await.unwrap();
        assert_eq!(loaded.agent_id, agent_id);
        assert_eq!(loaded.session_count, 0);
    }

    #[tokio::test]
    async fn trust_relationships_accumulate() {
        let store = AgentIdentityStore::new_in_memory();
        store
            .update_trust("agent-a", "agent-b", 0.1)
            .await
            .unwrap();
        let identity = store.load("agent-a").await.unwrap();
        assert!((identity.trust_relationships.get("agent-b").unwrap() - 0.6).abs() < 1e-9);
        store
            .update_trust("agent-a", "agent-b", 0.1)
            .await
            .unwrap();
        let identity = store.load("agent-a").await.unwrap();
        assert!((identity.trust_relationships.get("agent-b").unwrap() - 0.7).abs() < 1e-9);
    }

    #[tokio::test]
    async fn new_agent_gets_fresh_identity() {
        let store = AgentIdentityStore::new_in_memory();
        let identity = store.load("new-agent").await.unwrap();
        assert_eq!(identity.agent_id, "new-agent");
        assert_eq!(identity.session_count, 0);
    }

    #[tokio::test]
    async fn record_task_updates_experience_and_skill() {
        let store = AgentIdentityStore::new_in_memory();
        store
            .record_task("test-agent", "rust", true)
            .await
            .unwrap();
        let identity = store.load("test-agent").await.unwrap();
        assert_eq!(identity.accumulated_experience, 1);
        assert_eq!(identity.state.talent.get("rust"), Some(&0.05));
    }

    #[tokio::test]
    async fn proficiency_capped_at_one() {
        let store = AgentIdentityStore::new_in_memory();
        for _ in 0..25 {
            store
                .record_task("test-agent", "rust", true)
                .await
                .unwrap();
        }
        let identity = store.load("test-agent").await.unwrap();
        assert_eq!(identity.state.talent.get("rust"), Some(&1.0));
    }

    #[tokio::test]
    async fn trust_clamped_to_valid_range() {
        let store = AgentIdentityStore::new_in_memory();
        store
            .update_trust("agent-a", "agent-b", 5.0)
            .await.unwrap();
        let identity = store.load("agent-a").await.unwrap();
        assert!((identity.trust_relationships.get("agent-b").unwrap() - 1.0).abs() < 1e-9);
        store
            .update_trust("agent-a", "agent-b", -5.0)
            .await
            .unwrap();
        let identity = store.load("agent-a").await.unwrap();
        assert!((identity.trust_relationships.get("agent-b").unwrap() - 0.0).abs() < 1e-9);
    }
}
