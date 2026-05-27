//! SubAgent spawning and result aggregation.
//!
//! SubAgents are lightweight child agents spawned to handle subtasks.  They
//! communicate with the parent via tokio channels and support timeout /
//! cancellation.
//!
//! # Lifecycle
//! 1. A [`SubAgent`] is constructed with a parent ID and [`AgentConfig`].
//! 2. Calling [`SubAgent::spawn`] creates a oneshot channel, launches a tokio
//!    task with the provided async handler, and returns a [`SubAgentHandle`].
//! 3. The caller awaits [`SubAgentHandle::await_result`] (optionally with a
//!    timeout) to retrieve the [`Message`] produced by the handler.
//!
//! # Pooling
//! [`SubAgentPool`] collects multiple handles and provides convenience
//! aggregators: collect all results, only successes, or merge text responses.
//!
//! # Cross-module relationships
//! - [`SubAgent`] does not depend on `AgentRuntime`; it only needs an
//!   `AgentConfig` and a user-provided handler.  This keeps it usable for
//!   both full-pipeline subagents and simple one-off computations.
//! - `FanOut` uses `SubAgentPool` internally for parallel Mixture-of-Agents
//!   execution.

use std::time::Duration;

// Dependency: core error types and agent/message primitives.
use clawz_core::{
    error::{ClawzError, Result},
    types::{
        agent::{AgentConfig, AgentStatus},
        message::Message,
    },
};
// Dependency: tokio async channels for parent-child result communication.
use tokio::sync::oneshot;
use uuid::Uuid;

// ── SubAgentHandle ─────────────────────────────────────────────────────────────

/// A handle to a running subagent.
///
/// The handle owns the oneshot receiver side of the result channel.
/// It can only be awaited once; after consumption the handle is defunct.
#[derive(Debug)]
pub struct SubAgentHandle {
    /// Unique subagent identifier (UUID v4).
    pub id: String,
    /// Agent ID of the parent that spawned this subagent.
    pub parent_id: String,
    /// Description of the subtask.
    pub task: String,
    /// Current runtime status (Running until the result is consumed).
    pub status: AgentStatus,
    /// Result receiver.  Wrapped in `Option` so we can `.take()` it on await,
    /// preventing double-consumption.
    result_rx: Option<oneshot::Receiver<Result<Message>>>,
}

impl SubAgentHandle {
    /// Await the subagent's result with an optional timeout.
    ///
    /// # Errors
    /// - Returns [`ClawzError::Internal`] if called twice (receiver already taken).
    /// - Returns [`ClawzError::Internal`] if the timeout fires.
    /// - Returns [`ClawzError::Internal`] if the sender side dropped without sending.
    pub async fn await_result(mut self, timeout: Option<Duration>) -> Result<Message> {
        let rx = self.result_rx.take().ok_or_else(|| {
            ClawzError::Internal("subagent result channel already consumed".into())
        })?;

        match timeout {
            Some(dur) => tokio::time::timeout(dur, rx)
                .await
                .map_err(|_| {
                    ClawzError::Internal(format!(
                        "subagent '{}' timed out after {:?}",
                        self.id, dur
                    ))
                })?
                .map_err(|_| {
                    ClawzError::Internal(format!("subagent '{}' result channel dropped", self.id))
                })?,
            None => rx.await.map_err(|_| {
                ClawzError::Internal(format!("subagent '{}' result channel dropped", self.id))
            })?,
        }
    }
}

// ── SubAgent ──────────────────────────────────────────────────────────────────

/// Spawns and manages subagent tasks.
///
/// `SubAgent` is intentionally cheap to construct: it holds only a parent
/// ID and an `AgentConfig` clone.  The actual work happens inside the
/// user-supplied handler, which runs in a detached tokio task.
pub struct SubAgent {
    /// ID of the agent that owns this subagent.
    parent_id: String,
    /// Configuration inherited from the parent (model, system prompt, etc.).
    config: AgentConfig,
}

impl SubAgent {
    /// Create a new subagent factory bound to `parent_id`.
    pub fn new(parent_id: impl Into<String>, config: AgentConfig) -> Self {
        Self {
            parent_id: parent_id.into(),
            config,
        }
    }

    /// Spawn the subagent to handle `task`, returning a handle.
    ///
    /// The subagent executes `handler` in a separate tokio task and sends the
    /// result back through a oneshot channel.
    ///
    /// # Type parameters
    /// - `F` — the handler closure.  Must be `Send + 'static` because it runs
    ///   in a spawned task.
    /// - `Fut` — the async output of the closure.
    pub fn spawn<F, Fut>(&self, task: impl Into<String>, handler: F) -> SubAgentHandle
    where
        F: FnOnce(AgentConfig, String) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Message>> + Send,
    {
        let task_str = task.into();
        let id = Uuid::new_v4().to_string();
        let config = self.config.clone();
        let task_clone = task_str.clone();

        let (tx, rx) = oneshot::channel::<Result<Message>>();

        // Spawn the work on the tokio runtime.  The task is detached; the
        // only way to observe its result is through the returned handle.
        tokio::spawn(async move {
            let result = handler(config, task_clone).await;
            let _ = tx.send(result);
        });

        SubAgentHandle {
            id,
            parent_id: self.parent_id.clone(),
            task: task_str,
            status: AgentStatus::Running,
            result_rx: Some(rx),
        }
    }

    /// Spawn a simple subagent that just returns a formatted result message.
    ///
    /// Useful for testing, mocking, or trivial aggregation steps where no
    /// actual LLM call is required.
    pub fn spawn_simple(
        &self,
        task: impl Into<String>,
        response_text: impl Into<String>,
    ) -> SubAgentHandle {
        let response = Message::assistant(response_text.into());
        let task_str = task.into();
        let _config = self.config.clone();
        self.spawn(task_str, move |_cfg, _task| {
            let msg = response;
            async move { Ok(msg) }
        })
    }
}

// ── SubAgentPool ──────────────────────────────────────────────────────────────

/// Manages a pool of subagent handles with result aggregation.
///
/// Collect handles via [`SubAgentPool::add`], then choose one of the
/// `collect_*` methods to await them.  The pool is consumed on collection
/// because it takes ownership of the handles.
pub struct SubAgentPool {
    /// Owned handles awaiting resolution.
    handles: Vec<SubAgentHandle>,
}

impl SubAgentPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            handles: Vec::new(),
        }
    }

    /// Add a handle to the pool.
    pub fn add(&mut self, handle: SubAgentHandle) {
        self.handles.push(handle);
    }

    /// Number of handles currently in the pool.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Returns `true` if the pool contains no handles.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Await all subagents, returning their results (errors preserved).
    ///
    /// Order of the returned vector matches the insertion order.
    pub async fn collect_all(self, timeout: Option<Duration>) -> Vec<Result<Message>> {
        let mut results = Vec::with_capacity(self.handles.len());
        for handle in self.handles {
            results.push(handle.await_result(timeout).await);
        }
        results
    }

    /// Await all subagents, returning only successful results.
    ///
    /// Errors are silently dropped; use [`SubAgentPool::collect_all`] if
    /// you need to inspect failures.
    pub async fn collect_successful(self, timeout: Option<Duration>) -> Vec<Message> {
        self.collect_all(timeout)
            .await
            .into_iter()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// Await all and aggregate text responses into a single string.
    ///
    /// The separator is inserted between each response.  Empty / non-text
    /// responses are skipped.
    pub async fn aggregate_text(self, separator: &str, timeout: Option<Duration>) -> String {
        self.collect_successful(timeout)
            .await
            .iter()
            .filter_map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join(separator)
    }
}

impl Default for SubAgentPool {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::agent::AgentConfig;

    fn make_config() -> AgentConfig {
        AgentConfig::new("subagent", "gpt-4")
    }

    #[tokio::test]
    async fn test_spawn_simple_returns_message() {
        let sa = SubAgent::new("parent-1", make_config());
        let handle = sa.spawn_simple("analyze data", "Analysis complete");
        let msg = handle.await_result(None).await.unwrap();
        assert_eq!(msg.content.as_text().unwrap(), "Analysis complete");
    }

    #[tokio::test]
    async fn test_spawn_with_handler() {
        let sa = SubAgent::new("parent-1", make_config());
        let handle = sa.spawn("compute", |_cfg, task| async move {
            Ok(Message::assistant(format!("done: {task}")))
        });
        let msg = handle.await_result(None).await.unwrap();
        assert!(msg.content.as_text().unwrap().contains("done: compute"));
    }

    #[tokio::test]
    async fn test_pool_collect_all() {
        let sa = SubAgent::new("parent-1", make_config());
        let mut pool = SubAgentPool::new();
        pool.add(sa.spawn_simple("t1", "result 1"));
        pool.add(sa.spawn_simple("t2", "result 2"));

        let results = pool.collect_all(None).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_ok()));
    }

    #[tokio::test]
    async fn test_pool_aggregate_text() {
        let sa = SubAgent::new("parent-1", make_config());
        let mut pool = SubAgentPool::new();
        pool.add(sa.spawn_simple("t1", "alpha"));
        pool.add(sa.spawn_simple("t2", "beta"));

        let text = pool.aggregate_text(" | ", None).await;
        // Order depends on task scheduling; just check both appear.
        assert!(text.contains("alpha"));
        assert!(text.contains("beta"));
    }

    #[tokio::test]
    async fn test_timeout_returns_error() {
        let sa = SubAgent::new("parent-1", make_config());
        let handle = sa.spawn("slow", |_cfg, _task| async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(Message::assistant("late"))
        });
        let result = handle.await_result(Some(Duration::from_millis(10))).await;
        assert!(result.is_err());
    }
}
