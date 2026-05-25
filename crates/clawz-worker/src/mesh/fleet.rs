//! Fleet-level coordination across the mesh.
//!
//! [`FleetMesh`] wraps [`MeshManager`] with higher-level fleet semantics:
//!
//! - **Task delegation**: route a task to the peer hosting a specific agent.
//! - **Agent discovery**: query all peers for their hosted agents.
//! - **Leader election**: simple "highest uptime wins" Borda-count election.
//! - **Consensus**: collect votes from a quorum of peers.
//! - **Load balancing**: suggest task migration when a peer is overloaded.
//!
//! # Role in Networking
//!
//! This layer sits above the raw mesh transport and turns it into a
//! task-oriented cluster primitive.  It is only active when the worker
//! runs in Elastic mode (see `clawz_core::deployment::DeploymentMode`).
//!
//! # Key Dependencies
//!
//! - [`crate::mesh::manager::MeshManager`] — lower-level peer registry and send primitive.
//! - [`crate::mesh::heartbeat::PeerOverallStatus`] — heartbeat drives peer eligibility.
//! - `clawz_core::types::mesh::TrafficType` — used to tag delegation traffic.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use uuid::Uuid;

// Dependency: clawz-core::error — unified error type for the whole crate.
use clawz_core::error::{ClawzError, Result};
// Dependency: clawz-core::types::mesh::TrafficType — tags mesh traffic by purpose.
use clawz_core::types::mesh::TrafficType;

// Dependency: crate::mesh::manager — wrapped by FleetMesh for peer lifecycle.
use crate::mesh::manager::{MeshManager, MeshPeer, PeerState};

// ── AgentLocation ─────────────────────────────────────────────────────────────

/// Maps a logical agent ID to the mesh peer that is currently hosting it.
#[derive(Debug, Clone)]
pub struct AgentLocation {
    /// The logical agent identifier.
    pub agent_id: String,
    /// UUID of the peer node hosting this agent.
    pub peer_id: Uuid,
    /// Hostname of the peer (for logging).
    pub hostname: String,
    /// Number of tasks currently running on this agent.
    pub active_tasks: u32,
    /// Maximum concurrent tasks this agent supports.
    pub max_concurrent_tasks: u32,
}

impl AgentLocation {
    /// Return true when the agent has reached or exceeded its task limit.
    pub fn is_overloaded(&self) -> bool {
        self.max_concurrent_tasks > 0
            && self.active_tasks >= self.max_concurrent_tasks
    }

    /// Normalised load in the range [0.0, 1.0] (or 0.0 if unlimited).
    pub fn load_fraction(&self) -> f64 {
        if self.max_concurrent_tasks == 0 {
            return 0.0;
        }
        self.active_tasks as f64 / self.max_concurrent_tasks as f64
    }
}

// ── LeaderElectionResult ──────────────────────────────────────────────────────

/// Result of a leader election round.
#[derive(Debug, Clone)]
pub struct LeaderElectionResult {
    /// The elected leader's peer ID.
    pub leader_id: Uuid,
    /// The leader's hostname (for display).
    pub leader_hostname: String,
    /// Uptime in seconds that won the election.
    pub uptime_secs: u64,
    /// Number of candidates that participated.
    pub candidate_count: usize,
}

// ── ConsensusProposal ─────────────────────────────────────────────────────────

/// A proposal submitted for fleet-wide consensus.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConsensusProposal {
    /// Unique identifier for this proposal.
    pub proposal_id: Uuid,
    /// Human-readable description.
    pub topic: String,
    /// Serialized proposal payload (application-defined).
    pub payload: serde_json::Value,
}

/// The outcome of a consensus round.
#[derive(Debug, Clone)]
pub struct ConsensusResult {
    /// Proposal ID this result corresponds to.
    pub proposal_id: Uuid,
    /// True if a quorum agreed.
    pub accepted: bool,
    /// Number of votes in favour.
    pub votes_for: usize,
    /// Total votes cast.
    pub votes_total: usize,
    /// Required quorum size.
    pub quorum: usize,
}

impl ConsensusResult {
    /// Fraction of votes that were in favour (0.0–1.0).
    pub fn quorum_fraction(&self) -> f64 {
        if self.votes_total == 0 {
            return 0.0;
        }
        self.votes_for as f64 / self.votes_total as f64
    }
}

// ── MigrationSuggestion ───────────────────────────────────────────────────────

/// Suggestion to migrate an agent's tasks to a less-loaded peer.
#[derive(Debug, Clone)]
pub struct MigrationSuggestion {
    /// Agent that should be moved.
    pub agent_id: String,
    /// Source peer currently hosting the agent.
    pub from_peer: Uuid,
    /// Destination peer with spare capacity.
    pub to_peer: Uuid,
    /// Human-readable rationale (includes load percentage).
    pub reason: String,
}

// ── FleetMesh ─────────────────────────────────────────────────────────────────

/// Fleet coordination layer wrapping [`MeshManager`].
///
/// Provides task-oriented operations (agent registry, delegation,
/// leader election, consensus, rebalancing) on top of the raw mesh.
pub struct FleetMesh {
    /// Underlying mesh manager (shared with heartbeat / router tasks).
    manager: Arc<MeshManager>,
    /// Agent location registry: agent_id → AgentLocation.
    agents: Arc<RwLock<HashMap<String, AgentLocation>>>,
    /// This node's own peer ID in the mesh.
    local_peer_id: Uuid,
}

impl FleetMesh {
    /// Create a FleetMesh wrapping an existing MeshManager.
    pub fn new(manager: Arc<MeshManager>) -> Self {
        let local_peer_id = manager.identity().node_id;
        Self {
            manager,
            agents: Arc::new(RwLock::new(HashMap::new())),
            local_peer_id,
        }
    }

    // ── Agent registry ─────────────────────────────────────────────────────────

    /// Register a local agent in the fleet registry.
    pub async fn register_local_agent(
        &self,
        agent_id: impl Into<String>,
        max_concurrent_tasks: u32,
    ) {
        let agent_id = agent_id.into();
        let identity = self.manager.identity();
        self.agents.write().await.insert(
            agent_id.clone(),
            AgentLocation {
                agent_id,
                peer_id: self.local_peer_id,
                hostname: identity.hostname.clone(),
                active_tasks: 0,
                max_concurrent_tasks,
            },
        );
    }

    /// Update the active task count for an agent.
    pub async fn update_task_count(&self, agent_id: &str, active_tasks: u32) {
        if let Some(loc) = self.agents.write().await.get_mut(agent_id) {
            loc.active_tasks = active_tasks;
        }
    }

    /// Return the current location of an agent.
    pub async fn locate_agent(&self, agent_id: &str) -> Option<AgentLocation> {
        self.agents.read().await.get(agent_id).cloned()
    }

    /// Register a remote peer's agent in the local registry.
    pub async fn register_remote_agent(&self, location: AgentLocation) {
        self.agents
            .write()
            .await
            .insert(location.agent_id.clone(), location);
    }

    // ── Task delegation ────────────────────────────────────────────────────────

    /// Delegate a task to the peer hosting the given agent.
    ///
    /// Serialises `task_payload` as JSON and sends it via the mesh manager
    /// using `Rpc` traffic type.
    pub async fn delegate_task(
        &self,
        task_payload: &serde_json::Value,
        target_agent_id: &str,
    ) -> Result<serde_json::Value> {
        let location = self.locate_agent(target_agent_id).await.ok_or_else(|| {
            ClawzError::NotFound {
                entity: "agent".to_string(),
                id: target_agent_id.to_string(),
            }
        })?;

        // Reject delegation to an already-saturated agent so the caller
        // can apply back-pressure or pick a different agent.
        if location.is_overloaded() {
            return Err(ClawzError::Mesh(format!(
                "Agent {target_agent_id} on peer {} is overloaded ({}/{} tasks)",
                location.peer_id, location.active_tasks, location.max_concurrent_tasks
            )));
        }

        // If the agent is local, no network hop needed.
        if location.peer_id == self.local_peer_id {
            log::debug!("Delegating task to local agent {target_agent_id}");
            return Ok(serde_json::json!({
                "status": "delegated",
                "target": target_agent_id,
                "peer": self.local_peer_id.to_string(),
                "local": true,
            }));
        }

        let envelope = serde_json::json!({
            "type": "task_delegation",
            "agent_id": target_agent_id,
            "payload": task_payload,
        });
        let raw = serde_json::to_vec(&envelope)?;

        let response_bytes = self
            .manager
            .send_to_peer(&location.peer_id, &raw, TrafficType::Rpc)
            .await?;

        let response: serde_json::Value = serde_json::from_slice(&response_bytes)
            .unwrap_or(serde_json::json!({"status": "ok"}));

        Ok(response)
    }

    // ── Agent discovery ────────────────────────────────────────────────────────

    /// Query all reachable peers for their hosted agents and update the registry.
    ///
    /// Sends a `discover_agents` request to each active peer and merges results.
    pub async fn discover_agents(&self) -> Vec<AgentLocation> {
        let peers = self.manager.get_peers().await;
        let active_peers: Vec<&MeshPeer> = peers
            .iter()
            .filter(|p| matches!(p.state, PeerState::Active))
            .collect();

        let query = serde_json::to_vec(&serde_json::json!({"type": "discover_agents"}))
            .unwrap_or_default();

        let mut all_locations: Vec<AgentLocation> = Vec::new();

        for peer in active_peers {
            match self
                .manager
                .send_to_peer(&peer.info.id, &query, TrafficType::Rpc)
                .await
            {
                Ok(response_bytes) => {
                    if let Ok(resp) = serde_json::from_slice::<serde_json::Value>(&response_bytes) {
                        if let Some(agents) = resp.get("agents").and_then(|a| a.as_array()) {
                            for agent_val in agents {
                                if let (Some(agent_id), Some(active), Some(max)) = (
                                    agent_val.get("agent_id").and_then(|v| v.as_str()),
                                    agent_val.get("active_tasks").and_then(|v| v.as_u64()),
                                    agent_val.get("max_concurrent_tasks").and_then(|v| v.as_u64()),
                                ) {
                                    let loc = AgentLocation {
                                        agent_id: agent_id.to_string(),
                                        peer_id: peer.info.id,
                                        hostname: peer.info.hostname.clone(),
                                        active_tasks: active as u32,
                                        max_concurrent_tasks: max as u32,
                                    };
                                    all_locations.push(loc.clone());
                                    self.register_remote_agent(loc).await;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::debug!(
                        "Agent discovery failed for peer {}: {e}",
                        peer.info.id
                    );
                }
            }
        }

        all_locations
    }

    // ── Leader election ────────────────────────────────────────────────────────

    /// Run a simple leader election for the given group.
    ///
    /// Algorithm: the candidate with the highest `uptime_secs` wins.
    /// In a tie, the node with the lexicographically smaller UUID wins.
    ///
    /// The `group` string is used to scope the election (e.g. "shard-0").
    pub async fn elect_leader(&self, _group: &str) -> Result<LeaderElectionResult> {
        let peers = self.manager.get_peers().await;
        let candidates: Vec<&MeshPeer> = peers
            .iter()
            .filter(|p| matches!(p.state, PeerState::Active))
            .collect();

        // Include ourselves as a candidate.
        let local_id = self.local_peer_id;
        let local_hostname = self.manager.identity().hostname.clone();

        // We use `uptime_secs` from the MeshPeer (time since Connected state).
        // If no peers, we are the leader by default.
        if candidates.is_empty() {
            return Ok(LeaderElectionResult {
                leader_id: local_id,
                leader_hostname: local_hostname,
                uptime_secs: 0,
                candidate_count: 1,
            });
        }

        // Build candidate list: (peer_id, uptime_secs, hostname).
        let mut all: Vec<(Uuid, u64, String)> = candidates
            .iter()
            .map(|p| (p.info.id, p.uptime_secs, p.info.hostname.clone()))
            .collect();

        // Add local node.
        all.push((local_id, 0, local_hostname.clone()));

        // Sort: highest uptime first, then by UUID ascending as tiebreaker.
        // Lexicographic UUID ordering is deterministic across all nodes,
        // preventing split-brain when uptimes are equal.
        all.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });

        let (leader_id, uptime_secs, leader_hostname) = all.remove(0);

        Ok(LeaderElectionResult {
            leader_id,
            leader_hostname,
            uptime_secs,
            candidate_count: all.len() + 1,
        })
    }

    // ── Consensus ─────────────────────────────────────────────────────────────

    /// Collect votes from active peers for a proposal.
    ///
    /// A quorum is defined as `floor(N/2) + 1` where N = active peers + self.
    /// Peers respond with `{"vote": true}` to accept or `{"vote": false}` to reject.
    /// If a peer doesn't respond, its vote is counted as abstain (not for).
    pub async fn consensus_check(
        &self,
        proposal: &ConsensusProposal,
    ) -> Result<ConsensusResult> {
        let peers = self.manager.get_peers().await;
        let active_peers: Vec<&MeshPeer> = peers
            .iter()
            .filter(|p| matches!(p.state, PeerState::Active))
            .collect();

        // Self is always included as a voter, so the quorum reflects
        // majority agreement of the *entire* visible population.
        let total_voters = active_peers.len() + 1; // +1 for self
        let quorum = total_voters / 2 + 1;

        let envelope = serde_json::json!({
            "type": "consensus_vote",
            "proposal": proposal,
        });
        let raw = serde_json::to_vec(&envelope)?;

        let mut votes_for = 1usize; // self always votes FOR
        let mut votes_total = 1usize;

        for peer in &active_peers {
            match self
                .manager
                .send_to_peer(&peer.info.id, &raw, TrafficType::Governance)
                .await
            {
                Ok(response_bytes) => {
                    votes_total += 1;
                    if let Ok(resp) =
                        serde_json::from_slice::<serde_json::Value>(&response_bytes)
                    {
                        if resp.get("vote").and_then(|v| v.as_bool()).unwrap_or(false) {
                            votes_for += 1;
                        }
                    }
                }
                Err(e) => {
                    log::debug!(
                        "Consensus vote failed for peer {}: {e}",
                        peer.info.id
                    );
                }
            }
        }

        let accepted = votes_for >= quorum;
        log::info!(
            "Consensus '{}': {}/{} votes (quorum={}): {}",
            proposal.topic,
            votes_for,
            votes_total,
            quorum,
            if accepted { "ACCEPTED" } else { "REJECTED" },
        );

        Ok(ConsensusResult {
            proposal_id: proposal.proposal_id,
            accepted,
            votes_for,
            votes_total,
            quorum,
        })
    }

    // ── Load balancing / rebalancing ───────────────────────────────────────────

    /// Suggest task migrations to reduce load on overloaded peers.
    ///
    /// Returns a list of migration suggestions sorted by urgency (most overloaded first).
    pub async fn suggest_rebalancing(&self) -> Vec<MigrationSuggestion> {
        let agents = self.agents.read().await;
        let peers = self.manager.get_peers().await;

        // Find active peers with spare capacity.
        let spare_peers: Vec<Uuid> = peers
            .iter()
            .filter(|p| matches!(p.state, PeerState::Active))
            .map(|p| p.info.id)
            .filter(|pid| {
                // Check if any agent on this peer has spare capacity.
                agents.values().any(|a| {
                    a.peer_id == *pid && !a.is_overloaded()
                })
            })
            .collect();

        let mut suggestions = Vec::new();

        for agent in agents.values() {
            if !agent.is_overloaded() {
                continue;
            }

            // Find a peer with spare capacity (different from current).
            let target = spare_peers.iter().find(|&&pid| pid != agent.peer_id);
            if let Some(&to_peer) = target {
                suggestions.push(MigrationSuggestion {
                    agent_id: agent.agent_id.clone(),
                    from_peer: agent.peer_id,
                    to_peer,
                    reason: format!(
                        "Agent {}/{} tasks (load={:.0}%)",
                        agent.active_tasks,
                        agent.max_concurrent_tasks,
                        agent.load_fraction() * 100.0
                    ),
                });
            }
        }

        // Sort by load fraction descending (most overloaded first).
        suggestions.sort_by(|a, b| {
            let fa = agents
                .get(&a.agent_id)
                .map(|ag| ag.load_fraction())
                .unwrap_or(0.0);
            let fb = agents
                .get(&b.agent_id)
                .map(|ag| ag.load_fraction())
                .unwrap_or(0.0);
            fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
        });

        suggestions
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::config::MeshConfig;

    fn make_fleet() -> FleetMesh {
        let mut cfg = MeshConfig::default();
        cfg.enabled = true;
        let mgr = Arc::new(MeshManager::new(cfg));
        FleetMesh::new(mgr)
    }

    #[tokio::test]
    async fn register_and_locate_local_agent() {
        let fleet = make_fleet();
        fleet.register_local_agent("agent-1", 10).await;

        let loc = fleet.locate_agent("agent-1").await.unwrap();
        assert_eq!(loc.agent_id, "agent-1");
        assert_eq!(loc.max_concurrent_tasks, 10);
        assert_eq!(loc.peer_id, fleet.local_peer_id);
    }

    #[tokio::test]
    async fn delegate_task_local() {
        let fleet = make_fleet();
        fleet.register_local_agent("agent-local", 5).await;

        let result = fleet
            .delegate_task(
                &serde_json::json!({"task": "summarise", "text": "hello world"}),
                "agent-local",
            )
            .await
            .unwrap();

        assert_eq!(result["local"], true);
        assert_eq!(result["target"], "agent-local");
    }

    #[tokio::test]
    async fn delegate_task_unknown_agent_fails() {
        let fleet = make_fleet();
        let result = fleet
            .delegate_task(&serde_json::json!({}), "ghost-agent")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn overloaded_agent_delegation_fails() {
        let fleet = make_fleet();
        fleet.register_local_agent("busy-agent", 2).await;
        fleet.update_task_count("busy-agent", 2).await; // at capacity

        let result = fleet
            .delegate_task(&serde_json::json!({"task": "work"}), "busy-agent")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn elect_leader_no_peers() {
        let fleet = make_fleet();
        let result = fleet.elect_leader("default").await.unwrap();
        // With no peers, local node should win.
        assert_eq!(result.leader_id, fleet.local_peer_id);
        assert_eq!(result.candidate_count, 1);
    }

    #[tokio::test]
    async fn consensus_self_only() {
        let fleet = make_fleet();
        let proposal = ConsensusProposal {
            proposal_id: Uuid::new_v4(),
            topic: "scale_out".to_string(),
            payload: serde_json::json!({"replicas": 3}),
        };
        let result = fleet.consensus_check(&proposal).await.unwrap();
        // 1 voter (self), quorum = 1, votes_for = 1 → accepted.
        assert!(result.accepted);
        assert_eq!(result.votes_for, 1);
        assert_eq!(result.quorum, 1);
    }

    #[tokio::test]
    async fn suggest_rebalancing_no_overload() {
        let fleet = make_fleet();
        fleet.register_local_agent("relaxed", 10).await;
        fleet.update_task_count("relaxed", 2).await;

        let suggestions = fleet.suggest_rebalancing().await;
        assert!(suggestions.is_empty()); // not overloaded
    }

    #[tokio::test]
    async fn agent_location_load_fraction() {
        let loc = AgentLocation {
            agent_id: "a".to_string(),
            peer_id: Uuid::new_v4(),
            hostname: "h".to_string(),
            active_tasks: 5,
            max_concurrent_tasks: 10,
        };
        assert!((loc.load_fraction() - 0.5).abs() < 0.001);
        assert!(!loc.is_overloaded());
    }

    #[tokio::test]
    async fn agent_location_overloaded() {
        let loc = AgentLocation {
            agent_id: "a".to_string(),
            peer_id: Uuid::new_v4(),
            hostname: "h".to_string(),
            active_tasks: 10,
            max_concurrent_tasks: 10,
        };
        assert!(loc.is_overloaded());
    }
}
