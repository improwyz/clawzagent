//! Tamper-evident audit logging with SHA-256 hash chaining.
//!
//! This module provides an append-only audit log where each entry is cryptographically
//! linked to the previous one via SHA-256 hashes. This hash chain creates a tamper-evident
//! structure: modifying any historical entry invalidates all subsequent hashes, making
//! undetected tampering computationally infeasible.
//!
//! # Hash Chain Mechanics
//!
//! The chain works as follows:
//! - The first entry uses `"genesis"` as its `prev_hash`.
//! - Each subsequent entry stores the `current_hash` of the previous entry as its `prev_hash`.
//! - The `current_hash` is computed as `SHA-256(prev_hash || canonical_entry_data)`.
//!
//! The canonical data string includes the entry's `id`, `timestamp`, `agent_id`, `action`,
//! and `result` joined by pipe characters. This deterministic serialization ensures the same
//! inputs always produce the same hash.
//!
//! # Verification
//!
//! Call [`AuditLogger::verify_chain`] to recompute the entire chain and detect any
//! modification to historical entries, their ordering, or the hash linkage.
//!
//! # Filtering
//!
//! Use [`AuditFilter`] and [`AuditLogger::get_entries`] to query entries by agent, action,
//! result, or time range. The filter uses a builder pattern for ergonomic composition.

use chrono::{DateTime, Utc};
use clawz_core::error::{ClawzError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, RwLock};

// ── AuditEntry ────────────────────────────────────────────────────────────────

/// A single tamper-evident audit log entry.
///
/// Each entry is linked to the previous one through SHA-256 hashes, forming a chain
/// that detects any modification to historical records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique identifier for this entry (UUID v4).
    pub id: String,

    /// UTC timestamp when the entry was created.
    pub timestamp: DateTime<Utc>,

    /// Identifier of the agent that performed the action.
    pub agent_id: String,

    /// Human-readable description of the action that was attempted.
    pub action: String,

    /// The governance decision for this action.
    pub result: AuditResult,

    /// Additional structured context (e.g. request parameters, error details).
    pub metadata: serde_json::Value,

    /// Hash of the previous entry's `current_hash`. For the first entry in the log,
    /// this is the literal string `"genesis"`.
    pub prev_hash: String,

    /// SHA-256 hash of `prev_hash || canonical_entry_data`.
    ///
    /// This hash binds this entry to the previous one. If any preceding entry is
    /// modified, the recomputed hash chain will diverge from the stored hashes,
    /// revealing tampering.
    pub current_hash: String,
}

/// Governance decision outcome for an audited action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditResult {
    /// The action was permitted to proceed.
    Allow,

    /// The action was blocked by policy.
    Deny,

    /// The action requires human review before proceeding.
    Review,

    /// An error occurred during evaluation (e.g. policy engine failure).
    Error,
}

impl std::fmt::Display for AuditResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditResult::Allow => write!(f, "allow"),
            AuditResult::Deny => write!(f, "deny"),
            AuditResult::Review => write!(f, "review"),
            AuditResult::Error => write!(f, "error"),
        }
    }
}

// ── AuditFilter ──────────────────────────────────────────────────────────────

/// Query filter for searching audit entries.
///
/// All fields are optional; only supplied criteria are applied. When multiple
/// criteria are set, they are combined with logical AND (all must match).
///
/// # Example
///
/// ```ignore
/// // AuditFilter requires chrono::Utc and std::time::Duration imports.
/// use clawz_worker::governance::audit::{AuditFilter, AuditResult};
/// use chrono::Utc;
/// use std::time::Duration;
///
/// let filter = AuditFilter::new()
///     .for_agent("agent-42")
///     .with_result(AuditResult::Deny)
///     .from(Utc::now() - Duration::hours(24));
/// ```
#[derive(Debug, Default, Clone)]
pub struct AuditFilter {
    /// Match only entries created by this agent.
    pub agent_id: Option<String>,

    /// Match only entries whose action contains this substring.
    pub action: Option<String>,

    /// Match only entries with this exact result.
    pub result: Option<AuditResult>,

    /// Match only entries created at or after this time.
    pub from: Option<DateTime<Utc>>,

    /// Match only entries created at or before this time.
    pub to: Option<DateTime<Utc>>,
}

impl AuditFilter {
    /// Create an empty filter that matches all entries.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict results to entries from the given agent.
    pub fn for_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Restrict results to entries whose action contains the given string.
    pub fn for_action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    /// Restrict results to entries with the given governance result.
    pub fn with_result(mut self, result: AuditResult) -> Self {
        self.result = Some(result);
        self
    }

    /// Restrict results to entries created at or after the given time.
    pub fn from(mut self, from: DateTime<Utc>) -> Self {
        self.from = Some(from);
        self
    }

    /// Restrict results to entries created at or before the given time.
    pub fn to(mut self, to: DateTime<Utc>) -> Self {
        self.to = Some(to);
        self
    }

    /// Check whether a single entry satisfies all active filter criteria.
    ///
    /// Returns `true` if the entry passes every supplied criterion, or if no
    /// criteria are set. The action check uses substring matching (`contains`).
    fn matches(&self, entry: &AuditEntry) -> bool {
        if let Some(ref aid) = self.agent_id {
            if &entry.agent_id != aid {
                return false;
            }
        }
        if let Some(ref action) = self.action {
            if !entry.action.contains(action.as_str()) {
                return false;
            }
        }
        if let Some(ref result) = self.result {
            if &entry.result != result {
                return false;
            }
        }
        if let Some(from) = self.from {
            if entry.timestamp < from {
                return false;
            }
        }
        if let Some(to) = self.to {
            if entry.timestamp > to {
                return false;
            }
        }
        true
    }
}

// ── AuditLogger ───────────────────────────────────────────────────────────────

/// Append-only tamper-evident audit logger.
///
/// `AuditLogger` maintains an in-memory sequence of [`AuditEntry`] records protected
/// by a SHA-256 hash chain. Each new entry links to the hash of the previous entry,
/// creating a chain where any modification to history is detectable via
/// [`verify_chain`](Self::verify_chain).
///
/// The logger is thread-safe: entries are stored behind an `Arc<RwLock<Vec<...>>>`.
///
/// # Hash Chain Details
///
/// When an entry is appended, its `current_hash` is computed as:
/// ```text
/// SHA-256(prev_hash || canonical)
/// ```
/// where `canonical` is a pipe-delimited string of the entry's core fields
/// (`id|timestamp|agent_id|action|result`). This binds the entry to its predecessor
/// and makes reordering detectable because each entry's `prev_hash` must match the
/// previous entry's `current_hash`.
pub struct AuditLogger {
    entries: Arc<RwLock<Vec<AuditEntry>>>,
}

impl AuditLogger {
    /// Create a new empty audit logger.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Compute `SHA-256(prev_hash || data)`.
    ///
    /// This helper concatenates the previous hash with the canonical entry data
    /// and returns the hex-encoded SHA-256 digest. It is the fundamental operation
    /// that links each entry to its predecessor in the chain.
    fn compute_hash(prev_hash: &str, data: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prev_hash.as_bytes());
        hasher.update(data.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Build the canonical data string used in hash computation.
    ///
    /// The format is `id|timestamp|agent_id|action|result`. This deterministic,
    /// pipe-delimited representation ensures that identical entry fields always
    /// yield identical hashes, regardless of JSON serialization choices or
    /// whitespace differences.
    fn canonical(
        id: &str,
        timestamp: &DateTime<Utc>,
        agent_id: &str,
        action: &str,
        result: &AuditResult,
    ) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            id,
            timestamp.to_rfc3339(),
            agent_id,
            action,
            result
        )
    }

    /// Append a new audit entry and return it.
    ///
    /// This method:
    /// 1. Locks the entry vector for writing.
    /// 2. Retrieves the `current_hash` of the last entry (or `"genesis"` if empty).
    /// 3. Generates a UUID v4 and captures the current UTC time.
    /// 4. Computes the canonical string and then the `current_hash`.
    /// 5. Pushes the new entry and returns a clone.
    ///
    /// The returned [`AuditEntry`] can be used by callers that need the generated
    /// `id` or `current_hash` immediately.
    pub fn append(
        &self,
        agent_id: impl Into<String>,
        action: impl Into<String>,
        result: AuditResult,
        metadata: serde_json::Value,
    ) -> AuditEntry {
        let mut entries = self.entries.write().unwrap();

        let prev_hash = entries
            .last()
            .map(|e| e.current_hash.clone())
            .unwrap_or_else(|| "genesis".to_string());

        let id = uuid::Uuid::new_v4().to_string();
        let timestamp = Utc::now();
        let agent_id_str = agent_id.into();
        let action_str = action.into();

        let canonical = Self::canonical(
            &id,
            &timestamp,
            &agent_id_str,
            &action_str,
            &result,
        );
        let current_hash = Self::compute_hash(&prev_hash, &canonical);

        let entry = AuditEntry {
            id,
            timestamp,
            agent_id: agent_id_str,
            action: action_str,
            result,
            metadata,
            prev_hash,
            current_hash,
        };

        log::debug!(
            "[audit] appended entry id={} agent={} action={}",
            entry.id,
            entry.agent_id,
            entry.action
        );

        entries.push(entry.clone());
        entry
    }

    /// Verify the integrity of the entire hash chain.
    ///
    /// This method walks the log sequentially, checking two invariants for every entry:
    ///
    /// 1. **Link invariant** — `entry.prev_hash` must equal the `current_hash` of the
    ///    preceding entry (or `"genesis"` for the first entry).
    /// 2. **Hash invariant** — Recomputing `SHA-256(entry.prev_hash || canonical)` must
    ///    match `entry.current_hash`.
    ///
    /// If either invariant fails at any position, a [`ClawzError::Governance`] is
    /// returned indicating the index and entry ID of the first broken link.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the chain has been tampered with or if an entry is missing
    /// its expected predecessor.
    pub fn verify_chain(&self) -> Result<()> {
        let entries = self.entries.read().unwrap();
        let mut expected_prev = "genesis".to_string();

        for (i, entry) in entries.iter().enumerate() {
            if entry.prev_hash != expected_prev {
                return Err(ClawzError::Governance(format!(
                    "audit chain broken at entry {} (id={}): prev_hash mismatch",
                    i, entry.id
                )));
            }

            let canonical = Self::canonical(
                &entry.id,
                &entry.timestamp,
                &entry.agent_id,
                &entry.action,
                &entry.result,
            );
            let expected_hash =
                Self::compute_hash(&entry.prev_hash, &canonical);

            if entry.current_hash != expected_hash {
                return Err(ClawzError::Governance(format!(
                    "audit chain broken at entry {} (id={}): hash mismatch",
                    i, entry.id
                )));
            }

            expected_prev = entry.current_hash.clone();
        }

        Ok(())
    }

    /// Query entries that match the given filter.
    ///
    /// The log is read-locked, each entry is tested against [`AuditFilter::matches`],
    /// and matching entries are cloned into the returned vector. This preserves the
    /// original log immutably.
    pub fn get_entries(&self, filter: &AuditFilter) -> Vec<AuditEntry> {
        self.entries
            .read()
            .unwrap()
            .iter()
            .filter(|e| filter.matches(e))
            .cloned()
            .collect()
    }

    /// Return the total number of entries in the log.
    pub fn entry_count(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// Count how many entries were created in the last hour.
    ///
    /// This is used by the PRISM monitoring subsystem to detect unusually high
    /// or low audit activity that might indicate misconfiguration or abuse.
    pub fn entries_last_hour(&self) -> u64 {
        let cutoff = Utc::now() - chrono::Duration::hours(1);
        self.entries
            .read()
            .unwrap()
            .iter()
            .filter(|e| e.timestamp >= cutoff)
            .count() as u64
    }

    /// Serialize all entries to a pretty-printed JSON string.
    ///
    /// # Errors
    ///
    /// Returns `Err(ClawzError::Serialization)` if JSON encoding fails.
    pub fn export_json(&self) -> Result<String> {
        let entries = self.entries.read().unwrap();
        serde_json::to_string_pretty(entries.as_slice())
            .map_err(|e| ClawzError::Serialization(e.to_string()))
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_and_verify_chain() {
        let logger = AuditLogger::new();
        logger.append("agent-1", "chat", AuditResult::Allow, serde_json::json!({}));
        logger.append("agent-1", "deploy", AuditResult::Deny, serde_json::json!({}));
        logger.append("agent-2", "read", AuditResult::Allow, serde_json::json!({}));

        assert_eq!(logger.entry_count(), 3);
        logger.verify_chain().unwrap();
    }

    #[test]
    fn test_tampered_chain_detected() {
        let logger = AuditLogger::new();
        logger.append("agent-1", "chat", AuditResult::Allow, serde_json::json!({}));
        logger.append("agent-1", "chat", AuditResult::Allow, serde_json::json!({}));

        // Tamper with the first entry's hash.
        {
            let mut entries = logger.entries.write().unwrap();
            entries[0].current_hash = "tampered_hash".into();
        }

        assert!(logger.verify_chain().is_err());
    }

    #[test]
    fn test_filter_by_agent() {
        let logger = AuditLogger::new();
        logger.append("agent-1", "chat", AuditResult::Allow, serde_json::json!({}));
        logger.append("agent-2", "deploy", AuditResult::Deny, serde_json::json!({}));

        let filter = AuditFilter::new().for_agent("agent-1");
        let entries = logger.get_entries(&filter);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].agent_id, "agent-1");
    }

    #[test]
    fn test_filter_by_result() {
        let logger = AuditLogger::new();
        logger.append("a1", "act1", AuditResult::Allow, serde_json::json!({}));
        logger.append("a2", "act2", AuditResult::Deny, serde_json::json!({}));
        logger.append("a3", "act3", AuditResult::Allow, serde_json::json!({}));

        let filter = AuditFilter::new().with_result(AuditResult::Allow);
        let entries = logger.get_entries(&filter);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_export_json() {
        let logger = AuditLogger::new();
        logger.append("a", "act", AuditResult::Allow, serde_json::json!({}));
        let json = logger.export_json().unwrap();
        assert!(json.contains("agent_id"));
    }

    #[test]
    fn test_entries_last_hour_count() {
        let logger = AuditLogger::new();
        logger.append("a", "act", AuditResult::Allow, serde_json::json!({}));
        logger.append("a", "act2", AuditResult::Deny, serde_json::json!({}));
        assert_eq!(logger.entries_last_hour(), 2);
    }
}
