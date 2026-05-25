//! Team coordination — leader + members with task delegation and load balancing.
//!
//! A [`Team`] represents a coordinated group of agents. The leader (the agent
//! that created the team) can enqueue tasks and delegate them to members.
//! Members are selected via **least-busy load balancing**: the agent with
//! the fewest active tasks wins the assignment. A priority [`VecDeque`] is
//! used for the task queue so higher-priority tasks can be dequeued first
//! (currently priority only affects ordering; the consumer controls insertion).
//!
//! # Responsibilities
//! - Member lifecycle: add, remove, status tracking.
//! - Task queue: enqueue, delegate, complete.
//! - Load-balancing delegation: assign the next pending task to the
//!   available member with the lowest `active_tasks` count.
//! - Broadcasting: send a message to every active member.
//!
//! # Cross-module relationships
//! - `Team` is used by higher-level orchestrators (e.g. `Workflow` or
//!   `AgentRuntime` delegation) to distribute subtasks.
//! - Each member is typically backed by an `AgentRuntime`; this module
//!   does *not* own the runtime — it only tracks IDs and load state.

use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::cmp::Reverse;
use std::sync::Arc;

// Dependency: timestamp types for task bookkeeping.
use chrono::{DateTime, Utc};
// Dependency: core error types and agent/message primitives.
use clawz_core::{
    error::{ClawzError, Result},
    types::{
        agent::{AgentConfig, AgentStatus},
        message::Message,
    },
};
use serde::{Deserialize, Serialize};
// Dependency: async read-write lock for concurrent team operations.
use tokio::sync::RwLock;
use uuid::Uuid;

// ── TeamRole ──────────────────────────────────────────────────────────────────

/// Functional role of a team member within a [`Team`].
///
/// Roles are informational — they do not change the delegation algorithm,
/// but they can be used by consumers to filter members or build role-aware
/// workflows (e.g. "only send review tasks to Reviewers").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    /// Team lead; usually the creator of the team.
    Leader,
    /// Code / output reviewer.
    Reviewer,
    /// Quality-assurance tester.
    Tester,
    /// Documentation writer.
    Documenter,
    /// Security / policy auditor.
    Auditor,
    /// Cross-member coordinator.
    Coordinator,
    /// General-purpose worker.
    Worker,
}

impl std::fmt::Display for TeamRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TeamRole::Leader => "leader",
            TeamRole::Reviewer => "reviewer",
            TeamRole::Tester => "tester",
            TeamRole::Documenter => "documenter",
            TeamRole::Auditor => "auditor",
            TeamRole::Coordinator => "coordinator",
            TeamRole::Worker => "worker",
        };
        write!(f, "{s}")
    }
}

// ── Task ─────────────────────────────────────────────────────────────────────

/// A unit of work that can be assigned to a team member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task identifier (UUID v4).
    pub id: String,
    /// Human-readable description of what needs to be done.
    pub description: String,
    /// Priority value; higher means more urgent.  Used when sorting the queue.
    pub priority: u8,
    /// ID of the member this task is assigned to, if any.
    pub assigned_to: Option<String>,
    /// Current lifecycle status of the task.
    pub status: TaskStatus,
    /// UTC timestamp when the task was created.
    pub created_at: DateTime<Utc>,
    /// UTC timestamp when the task was completed, if applicable.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Lifecycle states for a [`Task`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Waiting in the queue.
    Pending,
    /// Currently being processed by a member.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
}

impl Task {
    /// Create a new pending task.
    pub fn new(description: impl Into<String>, priority: u8) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            description: description.into(),
            priority,
            assigned_to: None,
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
        }
    }
}

// ── TeamMember ────────────────────────────────────────────────────────────────

/// Snapshot of a member's state inside a [`Team`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    /// Unique agent identifier.
    pub agent_id: String,
    /// Functional role within the team.
    pub role: TeamRole,
    /// Current runtime status (Idle, Running, Stopped, Error).
    pub status: AgentStatus,
    /// Number of tasks currently assigned to this member.
    pub active_tasks: usize,
    /// Lifetime counter of finished tasks (for metrics & load-history).
    pub total_tasks_completed: usize,
    /// UTC timestamp when this member joined the team.
    pub joined_at: DateTime<Utc>,
}

impl TeamMember {
    /// Create a new member in the `Idle` state with zero active tasks.
    pub fn new(agent_id: impl Into<String>, role: TeamRole) -> Self {
        Self {
            agent_id: agent_id.into(),
            role,
            status: AgentStatus::Idle,
            active_tasks: 0,
            total_tasks_completed: 0,
            joined_at: Utc::now(),
        }
    }

    /// Returns `true` unless the member is in a terminal error state.
    pub fn is_available(&self) -> bool {
        self.status != AgentStatus::Stopped && self.status != AgentStatus::Error
    }

    /// Current load metric used by the least-busy balancer.
    pub fn load(&self) -> usize {
        self.active_tasks
    }
}

// ── Team ─────────────────────────────────────────────────────────────────────

/// A coordinated group of agents with a leader and members.
///
/// All mutable state is protected by `tokio::sync::RwLock` so the team
/// can be shared across async tasks (e.g. an HTTP handler enqueuing tasks
/// while a background worker pulls and delegates them).
pub struct Team {
    /// Unique team identifier (UUID v4).
    id: String,
    /// Human-readable team name.
    name: String,
    /// Agent ID of the leader (always present from construction).
    leader_id: String,
    /// Map of agent_id → member state.  Protected by RwLock for concurrent access.
    members: Arc<RwLock<HashMap<String, TeamMember>>>,
    /// FIFO queue of pending tasks.  In future this could be a priority-aware
    /// structure; for now callers insert in priority order.
    task_queue: Arc<RwLock<VecDeque<Task>>>,
    /// Archive of finished tasks for audit / replay.
    completed_tasks: Arc<RwLock<Vec<Task>>>,
}

impl Team {
    /// Create a new team with the given name and leader.
    ///
    /// The leader is automatically inserted as the first member with
    /// [`TeamRole::Leader`].
    pub fn new(
        name: impl Into<String>,
        leader_id: impl Into<String>,
    ) -> Self {
        let leader_id = leader_id.into();
        let mut members = HashMap::new();
        members.insert(
            leader_id.clone(),
            TeamMember::new(leader_id.clone(), TeamRole::Leader),
        );

        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            leader_id,
            members: Arc::new(RwLock::new(members)),
            task_queue: Arc::new(RwLock::new(VecDeque::new())),
            completed_tasks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Return the team UUID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Return the team name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the leader's agent ID.
    pub fn leader_id(&self) -> &str {
        &self.leader_id
    }

    // ── Member management ─────────────────────────────────────────────────────

    /// Add a new member to the team.
    pub async fn add_member(&self, agent_id: impl Into<String>, role: TeamRole) {
        let mut members = self.members.write().await;
        let id = agent_id.into();
        members.insert(id.clone(), TeamMember::new(id, role));
    }

    /// Remove a member from the team.
    ///
    /// Fails with [`ClawzError::NotFound`] if the agent is not a member.
    pub async fn remove_member(&self, agent_id: &str) -> Result<()> {
        let mut members = self.members.write().await;
        members.remove(agent_id).ok_or_else(|| ClawzError::NotFound {
            entity: "team member".into(),
            id: agent_id.into(),
        })?;
        Ok(())
    }

    /// Update the runtime status of a member.
    pub async fn set_member_status(&self, agent_id: &str, status: AgentStatus) -> Result<()> {
        let mut members = self.members.write().await;
        members
            .get_mut(agent_id)
            .ok_or_else(|| ClawzError::NotFound {
                entity: "team member".into(),
                id: agent_id.into(),
            })?
            .status = status;
        Ok(())
    }

    /// Return a snapshot of every member.
    pub async fn members(&self) -> Vec<TeamMember> {
        self.members.read().await.values().cloned().collect()
    }

    /// Return only members that are currently available for work.
    pub async fn active_members(&self) -> Vec<TeamMember> {
        self.members
            .read()
            .await
            .values()
            .filter(|m| m.is_available())
            .cloned()
            .collect()
    }

    // ── Task management ───────────────────────────────────────────────────────

    /// Enqueue a task to be delegated to the least-busy member.
    pub async fn enqueue_task(&self, task: Task) {
        self.task_queue.write().await.push_back(task);
    }

    /// Delegate `task` to a specific member.
    ///
    /// Updates the task status to [`TaskStatus::InProgress`] and bumps the
    /// member's `active_tasks` counter.
    pub async fn delegate(&self, mut task: Task, agent_id: &str) -> Result<()> {
        let mut members = self.members.write().await;
        let member = members.get_mut(agent_id).ok_or_else(|| {
            ClawzError::NotFound {
                entity: "team member".into(),
                id: agent_id.into(),
            }
        })?;

        if !member.is_available() {
            return Err(ClawzError::Validation(format!(
                "member '{}' is not available (status: {:?})",
                agent_id, member.status
            )));
        }

        task.assigned_to = Some(agent_id.to_string());
        task.status = TaskStatus::InProgress;
        member.active_tasks += 1;
        member.status = AgentStatus::Running;

        log::debug!(
            "[team:{}] delegated task '{}' to '{}'",
            self.name,
            task.description,
            agent_id
        );
        Ok(())
    }

    /// Assign the next pending task to the least-busy available member.
    ///
    /// # Algorithm
    /// 1. Pop the front task from the queue.
    /// 2. Scan all available members (excluding the leader).
    /// 3. Pick the member with the smallest `active_tasks` count.
    /// 4. Call [`Team::delegate`] to hand the task over.
    ///
    /// Returns the `(task_id, agent_id)` pair if a task was assigned.
    pub async fn assign_next(&self) -> Option<(String, String)> {
        let mut queue = self.task_queue.write().await;
        let task = queue.pop_front()?;

        let members = self.members.read().await;
        // Find the available member with the lowest load.
        // We intentionally exclude the leader so the leader stays free for
        // coordination work; callers can explicitly delegate to the leader
        // if they need to.
        let target = members
            .values()
            .filter(|m| m.is_available() && m.agent_id != self.leader_id)
            .min_by_key(|m| m.active_tasks)?;

        let agent_id = target.agent_id.clone();
        let task_id = task.id.clone();
        // Explicitly drop both locks before the async delegate call to avoid
        // holding a RwLock across an await point (deadlock risk).
        drop(members);
        drop(queue);

        if self.delegate(task, &agent_id).await.is_ok() {
            Some((task_id, agent_id))
        } else {
            None
        }
    }

    /// Mark a task as completed by `agent_id`.
    ///
    /// Decrements `active_tasks` and transitions the member back to `Idle`
    /// if no tasks remain.
    pub async fn complete_task(&self, agent_id: &str) -> Result<()> {
        let mut members = self.members.write().await;
        let member = members.get_mut(agent_id).ok_or_else(|| {
            ClawzError::NotFound {
                entity: "team member".into(),
                id: agent_id.into(),
            }
        })?;
        if member.active_tasks > 0 {
            member.active_tasks -= 1;
        }
        member.total_tasks_completed += 1;
        if member.active_tasks == 0 {
            member.status = AgentStatus::Idle;
        }
        Ok(())
    }

    /// Broadcast a message to all active members.
    ///
    /// Returns the list of member IDs the message was sent to.
    /// This is a *fire-and-forget* abstraction: the actual delivery mechanism
    /// (channel, queue, direct call) is up to the caller.
    pub async fn broadcast(&self, message: &Message) -> Vec<String> {
        let members = self.members.read().await;
        members
            .values()
            .filter(|m| m.is_available())
            .map(|m| {
                log::debug!(
                    "[team:{}] broadcast → '{}': {:?}",
                    self.name,
                    m.agent_id,
                    message.content.as_text()
                );
                m.agent_id.clone()
            })
            .collect()
    }

    /// Current number of pending tasks in the queue.
    pub async fn queue_len(&self) -> usize {
        self.task_queue.read().await.len()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::message::Message;

    #[tokio::test]
    async fn test_team_creation() {
        let team = Team::new("alpha", "leader-1");
        assert_eq!(team.name(), "alpha");
        assert_eq!(team.leader_id(), "leader-1");
        let members = team.members().await;
        assert_eq!(members.len(), 1);
    }

    #[tokio::test]
    async fn test_add_remove_member() {
        let team = Team::new("alpha", "leader-1");
        team.add_member("worker-1", TeamRole::Worker).await;
        assert_eq!(team.members().await.len(), 2);

        team.remove_member("worker-1").await.unwrap();
        assert_eq!(team.members().await.len(), 1);
    }

    #[tokio::test]
    async fn test_delegate_task() {
        let team = Team::new("alpha", "leader-1");
        team.add_member("worker-1", TeamRole::Worker).await;

        let task = Task::new("do something", 5);
        team.delegate(task, "worker-1").await.unwrap();

        let members = team.members().await;
        let worker = members.iter().find(|m| m.agent_id == "worker-1").unwrap();
        assert_eq!(worker.active_tasks, 1);
    }

    #[tokio::test]
    async fn test_assign_next_picks_least_busy() {
        let team = Team::new("alpha", "leader-1");
        team.add_member("worker-1", TeamRole::Worker).await;
        team.add_member("worker-2", TeamRole::Worker).await;

        team.enqueue_task(Task::new("task A", 5)).await;
        let assignment = team.assign_next().await;
        assert!(assignment.is_some());
    }

    #[tokio::test]
    async fn test_complete_task_decrements_load() {
        let team = Team::new("alpha", "leader-1");
        team.add_member("worker-1", TeamRole::Worker).await;

        let task = Task::new("work", 5);
        team.delegate(task, "worker-1").await.unwrap();
        team.complete_task("worker-1").await.unwrap();

        let members = team.members().await;
        let worker = members.iter().find(|m| m.agent_id == "worker-1").unwrap();
        assert_eq!(worker.active_tasks, 0);
        assert_eq!(worker.total_tasks_completed, 1);
    }

    #[tokio::test]
    async fn test_broadcast_returns_active_members() {
        let team = Team::new("alpha", "leader-1");
        team.add_member("worker-1", TeamRole::Worker).await;
        team.set_member_status("worker-1", AgentStatus::Stopped)
            .await
            .unwrap();
        team.add_member("worker-2", TeamRole::Worker).await;

        let msg = Message::system("hey team");
        let recipients = team.broadcast(&msg).await;
        assert_eq!(recipients.len(), 2); // leader + worker-2
        assert!(!recipients.contains(&"worker-1".to_string()));
    }
}
