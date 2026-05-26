//! AgentIdentityStore — cross-session accumulated identity.
//!
//! Accumulates trust relationships, skill proficiencies, and experience
//! across sessions for each agent.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use clawz_core::error::ClawzError;

/// Accumulated identity for an agent across all sessions.
/// This is the "who I am" that persists after each session ends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentity {
    /// Unique identifier for this agent.
    pub agent_id: String,
    /// Map of agent_id → trust score [0,1].
    pub trust_relationships: HashMap<String, f64>,
    /// Total number of tasks completed across all sessions.
    pub accumulated_experience: u64,
    /// Map of skill_name → proficiency [0,1].
    pub skill_proficiencies: HashMap<String, f32>,
    /// Wall-clock timestamp of last seen.
    pub last_seen: DateTime<Utc>,
    /// Total number of sessions completed.
    pub session_count: u64,
}

impl AgentIdentity {
    /// Create a fresh identity for a new agent.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            trust_relationships: HashMap::new(),
            accumulated_experience: 0,
            skill_proficiencies: HashMap::new(),
            last_seen: Utc::now(),
            session_count: 0,
        }
    }

    /// Record a completed task and update skill proficiency.
    ///
    /// On success: increments experience and bumps skill proficiency by 0.05.
    /// On failure: only increments experience, proficiency unchanged.
    pub fn record_task(&mut self, skill_name: &str, success: bool) {
        self.accumulated_experience += 1;
        if success {
            let entry = self.skill_proficiencies.entry(skill_name.to_string()).or_insert(0.0);
            // Learning rate: +0.05 per successful task, capped at 1.0.
            *entry = (*entry + 0.05).min(1.0);
        }
        self.last_seen = Utc::now();
    }

    /// Update a trust relationship with another agent.
    ///
    /// Positive `delta` increases trust; negative decreases.
    /// Clamps trust score to [0, 1].
    pub fn update_trust(&mut self, other_agent_id: &str, delta: f64) {
        let entry = self
            .trust_relationships
            .entry(other_agent_id.to_string())
            .or_insert(0.5);
        *entry = (*entry + delta).clamp(0.0, 1.0);
        self.last_seen = Utc::now();
    }

    /// Increment the session counter.
    pub fn increment_session(&mut self) {
        self.session_count += 1;
        self.last_seen = Utc::now();
    }
}

/// Preference vector for an agent — used to personalize tool and governance choices.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PreferenceVector {
    /// Tool name → preference score [0,1].
    pub tool_preferences: HashMap<String, f32>,
    /// Governance policy name → preference score [0,1].
    pub governance_preferences: HashMap<String, f32>,
}

/// Backend storage for agent identities.
#[async_trait]
pub trait IdentityBackend: Send + Sync {
    /// Load the identity for `agent_id`. Returns `None` if not yet stored.
    async fn load(&self, agent_id: &str) -> Result<Option<AgentIdentity>, ClawzError>;

    /// Persist `identity` for its agent_id.
    async fn save(&self, identity: &AgentIdentity) -> Result<(), ClawzError>;
}

// ---------------------------------------------------------------------------
// In-memory backend — use for development and tests.
// Replace with a PostgreSQL-backed implementation in production.
// ---------------------------------------------------------------------------

/// In-memory identity backend using a shared `HashMap`.
#[derive(Debug, Default)]
pub struct InMemoryIdentityBackend {
    store: RwLock<HashMap<String, AgentIdentity>>,
}

impl InMemoryIdentityBackend {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl IdentityBackend for InMemoryIdentityBackend {
    async fn load(&self, agent_id: &str) -> Result<Option<AgentIdentity>, ClawzError> {
        let guard = self.store.read().await;
        Ok(guard.get(agent_id).cloned())
    }

    async fn save(&self, identity: &AgentIdentity) -> Result<(), ClawzError> {
        let mut guard = self.store.write().await;
        guard.insert(identity.agent_id.clone(), identity.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AgentIdentityStore — the main public API
// ---------------------------------------------------------------------------

/// In-memory identity store. Use this for now; replace with a
/// PostgreSQL-backed implementation in production.
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
    /// Construct an in-memory identity store.
    pub fn new_in_memory() -> Self {
        Self {
            backend: Arc::new(InMemoryIdentityBackend::new()),
        }
    }

    /// Construct with a custom backend.
    pub fn with_backend(backend: Arc<dyn IdentityBackend>) -> Self {
        Self { backend }
    }

    /// Load identity for `agent_id`, creating a fresh one if none exists.
    pub async fn load(&self, agent_id: &str) -> Result<AgentIdentity, ClawzError> {
        match self.backend.load(agent_id).await? {
            Some(identity) => Ok(identity),
            None => Ok(AgentIdentity::new(agent_id)),
        }
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
    async fn identity_store_persists_across_load_cycle() {
        let store = AgentIdentityStore::new_in_memory();

        let initial = AgentIdentity::new("agent-42");
        store.save(&initial).await.unwrap();

        let loaded = store.load("agent-42").await.unwrap();
        assert_eq!(loaded.agent_id, "agent-42");
        assert_eq!(loaded.session_count, 0);
    }

    #[tokio::test]
    async fn trust_relationships_accumulate() {
        let store = AgentIdentityStore::new_in_memory();

        // First session: alice updates trust to bob by +0.2
        let mut identity = AgentIdentity::new("alice");
        identity.update_trust("bob", 0.2);
        store.save(&identity).await.unwrap();

        // Second session: load and increase again
        let mut identity = store.load("alice").await.unwrap();
        identity.update_trust("bob", 0.2);
        store.save(&identity).await.unwrap();

        // Verify accumulated trust
        let identity = store.load("alice").await.unwrap();
        let bob_trust = identity.trust_relationships.get("bob").unwrap();
        // Default 0.5 + 0.2 + 0.2 = 0.9
        assert!((*bob_trust - 0.9).abs() < 1e-9);
    }

    #[tokio::test]
    async fn new_agent_gets_fresh_identity() {
        let store = AgentIdentityStore::new_in_memory();

        let identity = store.load("never-seen-before").await.unwrap();
        assert_eq!(identity.agent_id, "never-seen-before");
        assert_eq!(identity.accumulated_experience, 0);
        assert!(identity.skill_proficiencies.is_empty());
        assert_eq!(identity.session_count, 0);
    }

    #[tokio::test]
    async fn record_task_updates_experience_and_skill() {
        let store = AgentIdentityStore::new_in_memory();

        let mut identity = AgentIdentity::new("worker-1");
        identity.record_task("rust", true);
        identity.record_task("rust", true);
        identity.record_task("rust", false); // failure — still counts as experience
        store.save(&identity).await.unwrap();

        let identity = store.load("worker-1").await.unwrap();
        assert_eq!(identity.accumulated_experience, 3);
        let rust_proficiency = identity.skill_proficiencies.get("rust").unwrap();
        // 2 successes × 0.05 = 0.10
        assert!((*rust_proficiency - 0.10).abs() < 1e-9);
    }

    #[tokio::test]
    async fn proficiency_capped_at_one() {
        let store = AgentIdentityStore::new_in_memory();

        let mut identity = AgentIdentity::new("over-achiever");
        for _ in 0..100 {
            identity.record_task("max-skills", true);
        }
        store.save(&identity).await.unwrap();

        let identity = store.load("over-achiever").await.unwrap();
        let proficiency = identity.skill_proficiencies.get("max-skills").unwrap();
        assert!((*proficiency - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn trust_clamped_to_valid_range() {
        let mut identity = AgentIdentity::new("alice");
        identity.update_trust("bob", 10.0); // should clamp to 1.0
        identity.update_trust("charlie", -10.0); // should clamp to 0.0

        assert!((*identity.trust_relationships.get("bob").unwrap() - 1.0).abs() < 1e-9);
        assert!((*identity.trust_relationships.get("charlie").unwrap() - 0.0).abs() < 1e-9);
    }
}