//! Idempotency layer — prevents double-execution of agent actions.
//!
//! Architecture:
//! - [`IdempotencyKey`] — composite key (task_id + action_name + date + nonce)
//! - [`IdempotencyResult`] — three states: New, Cached(ToolResult), Expired
//! - [`IdempotencyStore`] — async trait for pluggable backends (defined in `clawz_core`)
//! - [`InMemoryIdempotencyStore`] — RwLock-wrapped HashMap with TTL eviction
//!
//! Wired into [`ExecuteToolsStep`](crate::runtime::steps::tools::ExecuteToolsStep)
//! before every tool invocation.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use clawz_core::traits::{
    IdempotencyKey, IdempotencyResult, IdempotencyStore as CoreIdempotencyStore,
};
use clawz_core::types::tool::ToolResult;

// ── InMemoryIdempotencyStore ──────────────────────────────────────────────────

/// In-process idempotency store backed by a `RwLock<HashMap>`.
///
/// Expiry is checked on every `check_and_record` call; expired entries are
/// silently purged at that time.
pub struct InMemoryIdempotencyStore {
    store: RwLock<HashMap<String, (DateTime<Utc>, ToolResult)>>,
    ttl: Duration,
}

impl InMemoryIdempotencyStore {
    /// Create a new store with the default TTL of 24 hours.
    pub fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(86400),
        }
    }

    /// Create a store with a custom TTL.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    /// Returns true if the given expiry time is past the configured TTL.
    fn is_expired(&self, stored_at: DateTime<Utc>) -> bool {
        let elapsed = Utc::now().signed_duration_since(stored_at);
        elapsed.num_seconds() as u64 >= self.ttl.as_secs()
    }
}

impl Default for InMemoryIdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

fn cache_key(key: &IdempotencyKey) -> String {
    format!(
        "{}:{}:{}:{}",
        key.task_id, key.action_name, key.date, key.nonce
    )
}

#[async_trait]
impl CoreIdempotencyStore for InMemoryIdempotencyStore {
    async fn check_and_record(
        &self,
        key: &IdempotencyKey,
        result: ToolResult,
    ) -> IdempotencyResult {
        let ck = cache_key(key);

        // Read-then-write: hold the read guard briefly to check for an existing entry.
        {
            let guard = self.store.read();
            if let Some((stored_at, cached_result)) = guard.get(&ck) {
                if !self.is_expired(*stored_at) {
                    return IdempotencyResult::Cached(cached_result.clone());
                }
                // Expired — fall through to overwrite.
            }
        }

        // Upgrade to write lock to record or update the entry.
        let mut guard = self.store.write();
        // Re-check: another coroutine may have written while we were upgrading.
        if let Some((stored_at, cached_result)) = guard.get(&ck) {
            if !self.is_expired(*stored_at) {
                return IdempotencyResult::Cached(cached_result.clone());
            }
            // Expired — remove it and signal expiry to caller.
            guard.remove(&ck);
            return IdempotencyResult::Expired;
        }
        guard.insert(ck, (Utc::now(), result));
        IdempotencyResult::New
    }

    async fn get(&self, key: &IdempotencyKey) -> Option<ToolResult> {
        let ck = cache_key(key);
        let guard = self.store.read();
        guard.get(&ck).and_then(|(stored_at, result)| {
            if self.is_expired(*stored_at) {
                None
            } else {
                Some(result.clone())
            }
        })
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::tool::ToolResult;

    fn make_result(id: &str) -> ToolResult {
        ToolResult::ok(id, r#"{"status":"ok"}"#)
    }

    #[tokio::test]
    async fn test_new_key_returns_new() {
        let store = InMemoryIdempotencyStore::new();
        let key = IdempotencyKey::new("task-1", "echo");
        let result = make_result("0");

        let res = store.check_and_record(&key, result).await;
        assert!(matches!(res, IdempotencyResult::New));
    }

    #[tokio::test]
    async fn test_same_key_twice_returns_cached() {
        let store = InMemoryIdempotencyStore::new();
        let key = IdempotencyKey::new("task-1", "echo");
        let result = make_result("0");

        let first = store.check_and_record(&key, result.clone()).await;
        assert!(matches!(first, IdempotencyResult::New));

        let second = store.check_and_record(&key, result).await;
        assert!(matches!(second, IdempotencyResult::Cached(_)));
    }

    #[tokio::test]
    async fn test_get_returns_cached_result() {
        let store = InMemoryIdempotencyStore::new();
        let key = IdempotencyKey::new("task-1", "echo");
        let result = make_result("99");

        store.check_and_record(&key, result).await;

        let cached = store.get(&key).await;
        assert!(cached.is_some());
    }

    #[tokio::test]
    async fn test_expired_key_returns_expired() {
        // Use with_date_and_nonce to get a stable key. We test expiry by
        // verifying that a second call with TTL=0 returns Expired, not Cached.
        // The TTL=0 store should treat any stored entry as immediately expired.
        let store = InMemoryIdempotencyStore::with_ttl(Duration::ZERO);
        let key = IdempotencyKey::with_date_and_nonce("task-1", "echo", "2026-05-26", "nonce-x");
        let result = make_result("0");

        // First call — no entry exists, records it as New.
        let first = store.check_and_record(&key, result).await;
        assert!(matches!(first, IdempotencyResult::New));

        // Second call — entry exists but TTL=0 means it's expired.
        // The store should return Expired (not Cached) since we're past TTL.
        // Note: In a real async context, even a tiny await would advance time.
        // We simulate time-passage by inserting via get() then checking expiry.
        let res = store.check_and_record(&key, make_result("1")).await;
        assert!(matches!(res, IdempotencyResult::Expired));
    }

    #[tokio::test]
    async fn test_different_keys_are_independent() {
        let store = InMemoryIdempotencyStore::new();

        let key_a = IdempotencyKey::new("task-a", "echo");
        let key_b = IdempotencyKey::new("task-b", "echo");

        store.check_and_record(&key_a, make_result("a")).await;
        store.check_and_record(&key_b, make_result("b")).await;

        // Both should be independently cached.
        assert!(store.get(&key_a).await.is_some());
        assert!(store.get(&key_b).await.is_some());
    }

    #[tokio::test]
    async fn test_get_missing_key_returns_none() {
        let store = InMemoryIdempotencyStore::new();
        let key = IdempotencyKey::new("nonexistent", "echo");
        assert!(store.get(&key).await.is_none());
    }
}
