//! Multi-agent blackboard: thread-safe shared state with change notifications.
//!
//! Agents can `post` values to named keys, `read` the current value, and
//! `subscribe` to real-time change events via a `tokio::sync::broadcast`
//! channel.  A complete change history is also kept per key.
//!
//! This is inspired by the classic AI blackboard architecture: multiple
//! agents coordinate by writing to and reading from a shared workspace.
//!
//! // Dependency: `clawz_core::error::ClawzError` for missing-key errors.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};

use clawz_core::error::{ClawzError, Result};

// ── BlackboardEntry ───────────────────────────────────────────────────────────

/// A single entry in the blackboard's history for a given key.
#[derive(Debug, Clone)]
pub struct BlackboardEntry {
    /// The agent that posted this value.
    pub agent_id: String,
    /// The key under which the value was posted.
    pub key: String,
    /// The posted value.
    pub value: Value,
    /// When the value was posted.
    pub timestamp: DateTime<Utc>,
}

// ── ChangeEvent ───────────────────────────────────────────────────────────────

/// Broadcast event emitted whenever a key is updated.
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    /// Agent that performed the update.
    pub agent_id: String,
    /// Key that was updated.
    pub key: String,
    /// New value.
    pub value: Value,
    /// When the change occurred.
    pub timestamp: DateTime<Utc>,
}

// ── Inner key state ───────────────────────────────────────────────────────────

/// Mutable state kept for each key on the blackboard.
struct KeyState {
    /// Most recently posted value.
    current: Value,
    /// Append-only history of changes.
    history: Vec<BlackboardEntry>,
    /// Broadcast sender for this key.
    tx: broadcast::Sender<ChangeEvent>,
}

impl KeyState {
    fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            current: Value::Null,
            history: Vec::new(),
            tx,
        }
    }
}

// ── Blackboard ────────────────────────────────────────────────────────────────

/// Shared blackboard for multi-agent coordination.
///
/// Thread-safe — clone the `Arc<Blackboard>` to share across tasks.
/// Each key gets its own broadcast channel so subscribers only receive
/// events for the key they care about.
pub struct Blackboard {
    /// Key → per-key state (current value + history + broadcast channel).
    state: Arc<RwLock<HashMap<String, KeyState>>>,
    /// How many events each per-key broadcast channel can buffer.
    channel_capacity: usize,
}

impl Blackboard {
    pub fn new() -> Self {
        Self::with_channel_capacity(256)
    }

    pub fn with_channel_capacity(channel_capacity: usize) -> Self {
        Self {
            state: Arc::new(RwLock::new(HashMap::new())),
            channel_capacity,
        }
    }

    // ── Write ─────────────────────────────────────────────────────────────────

    /// Post a new value for `key` on behalf of `agent_id`.
    ///
    /// Overwrites the current value, appends to history, and broadcasts the
    /// change event to all active subscribers.
    pub async fn post(&self, agent_id: &str, key: &str, value: Value) -> Result<()> {
        let timestamp = Utc::now();
        let event = ChangeEvent {
            agent_id: agent_id.to_string(),
            key: key.to_string(),
            value: value.clone(),
            timestamp,
        };

        let mut guard = self.state.write().await;
        let key_state = guard
            .entry(key.to_string())
            .or_insert_with(|| KeyState::new(self.channel_capacity));

        key_state.current = value.clone();
        key_state.history.push(BlackboardEntry {
            agent_id: agent_id.to_string(),
            key: key.to_string(),
            value,
            timestamp,
        });

        // Ignore SendError – it just means no subscribers are currently listening.
        let _ = key_state.tx.send(event);

        Ok(())
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    /// Return the current value of `key`, or `None` if the key does not exist.
    pub async fn read(&self, key: &str) -> Option<Value> {
        let guard = self.state.read().await;
        guard.get(key).map(|ks| ks.current.clone())
    }

    /// Return the current value of `key` or a `NotFound` error.
    pub async fn read_required(&self, key: &str) -> Result<Value> {
        self.read(key).await.ok_or_else(|| ClawzError::NotFound {
            entity: "blackboard key".into(),
            id: key.to_string(),
        })
    }

    // ── Subscribe ─────────────────────────────────────────────────────────────

    /// Subscribe to changes on `key`.
    ///
    /// Returns a `broadcast::Receiver`; callers should call `.recv().await` in
    /// a loop to receive `ChangeEvent` notifications.
    ///
    /// If `key` does not yet exist, it is created (with a `Null` current value)
    /// so that the subscriber is ready before the first write.
    pub async fn subscribe(&self, key: &str) -> broadcast::Receiver<ChangeEvent> {
        let mut guard = self.state.write().await;
        let key_state = guard
            .entry(key.to_string())
            .or_insert_with(|| KeyState::new(self.channel_capacity));
        key_state.tx.subscribe()
    }

    // ── History ───────────────────────────────────────────────────────────────

    /// Return up to `limit` most recent change records for `key`.
    pub async fn history(&self, key: &str, limit: usize) -> Vec<BlackboardEntry> {
        let guard = self.state.read().await;
        match guard.get(key) {
            None => vec![],
            Some(ks) => {
                let start = ks.history.len().saturating_sub(limit);
                ks.history[start..].to_vec()
            }
        }
    }

    // ── Utility ───────────────────────────────────────────────────────────────

    /// Return all keys currently stored on the blackboard.
    pub async fn keys(&self) -> Vec<String> {
        let guard = self.state.read().await;
        guard.keys().cloned().collect()
    }

    /// Delete a key and its entire history.
    pub async fn delete(&self, key: &str) -> bool {
        let mut guard = self.state.write().await;
        guard.remove(key).is_some()
    }
}

impl Default for Blackboard {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_post_and_read() {
        let bb = Blackboard::new();
        bb.post("agent-1", "status", serde_json::json!("running"))
            .await
            .unwrap();
        let val = bb.read("status").await;
        assert_eq!(val, Some(serde_json::json!("running")));
    }

    #[tokio::test]
    async fn test_read_missing_returns_none() {
        let bb = Blackboard::new();
        assert!(bb.read("missing_key").await.is_none());
    }

    #[tokio::test]
    async fn test_read_required_error_on_missing() {
        let bb = Blackboard::new();
        let result = bb.read_required("nope").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_history_records_changes() {
        let bb = Blackboard::new();
        bb.post("a1", "counter", serde_json::json!(1)).await.unwrap();
        bb.post("a1", "counter", serde_json::json!(2)).await.unwrap();
        bb.post("a2", "counter", serde_json::json!(3)).await.unwrap();

        let hist = bb.history("counter", 10).await;
        assert_eq!(hist.len(), 3);
        assert_eq!(hist[0].value, serde_json::json!(1));
        assert_eq!(hist[2].value, serde_json::json!(3));
        assert_eq!(hist[2].agent_id, "a2");
    }

    #[tokio::test]
    async fn test_history_limit() {
        let bb = Blackboard::new();
        for i in 0..10u64 {
            bb.post("a1", "key", serde_json::json!(i)).await.unwrap();
        }
        let hist = bb.history("key", 3).await;
        assert_eq!(hist.len(), 3);
        assert_eq!(hist[2].value, serde_json::json!(9u64));
    }

    #[tokio::test]
    async fn test_subscribe_receives_events() {
        let bb = Arc::new(Blackboard::new());
        let mut rx = bb.subscribe("temperature").await;

        let bb2 = Arc::clone(&bb);
        let task = tokio::spawn(async move {
            bb2.post("sensor-1", "temperature", serde_json::json!(42.5))
                .await
                .unwrap();
        });

        task.await.unwrap();
        let event = rx.recv().await.unwrap();
        assert_eq!(event.key, "temperature");
        assert_eq!(event.value, serde_json::json!(42.5));
        assert_eq!(event.agent_id, "sensor-1");
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bb = Arc::new(Blackboard::new());
        let mut rx1 = bb.subscribe("signal").await;
        let mut rx2 = bb.subscribe("signal").await;

        bb.post("src", "signal", serde_json::json!("ping"))
            .await
            .unwrap();

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.value, serde_json::json!("ping"));
        assert_eq!(e2.value, serde_json::json!("ping"));
    }

    #[tokio::test]
    async fn test_delete() {
        let bb = Blackboard::new();
        bb.post("a", "k", serde_json::json!(1)).await.unwrap();
        assert!(bb.delete("k").await);
        assert!(bb.read("k").await.is_none());
        assert!(!bb.delete("k").await); // already gone
    }

    #[tokio::test]
    async fn test_keys() {
        let bb = Blackboard::new();
        bb.post("a", "k1", serde_json::json!(1)).await.unwrap();
        bb.post("a", "k2", serde_json::json!(2)).await.unwrap();
        let mut keys = bb.keys().await;
        keys.sort();
        assert_eq!(keys, vec!["k1", "k2"]);
    }
}
