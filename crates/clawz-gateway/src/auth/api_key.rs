//! API key lifecycle management: generation, hashing, validation, and records.
//!
//! This module provides the data model and logic for long-lived API keys used
//! by automated clients and agents. Keys are never stored in plain text;
//! only SHA-256 hashes are kept in [`ApiKeyRecord`] so that a leaked database
//! does not expose usable credentials.
//!
//! # Design notes
//! - Key generation mixes multiple UUID v4 values and subsecond timestamps to
//!   gather entropy without requiring the `rand` crate directly.
//! - Validation checks revocation status and optional expiry windows.
//!
//! # Key dependencies
//! - `sha2` / `base64` — cryptographic hashing and URL-safe encoding.
//! - `chrono` — expiry timestamp handling.
//! - `serde` — serialisable record structure.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn default_tenant_id() -> String {
    std::env::var("CLAWZ_TENANT_ID").unwrap_or_else(|_| "default".to_string())
}

/// Database (or environment) record representing a known API key.
///
/// Only the SHA-256 hash of the key is persisted; the raw key is ephemeral.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    /// Unique record identifier (UUID v4).
    pub id: String,
    /// Hex-encoded SHA-256 hash of the raw API key.
    pub key_hash: String,
    /// Identity that owns this key. Used to populate [`AuthContext`].
    pub user_id: String,
    /// Tenant scope for multi-tenant isolation.
    #[serde(default = "default_tenant_id")]
    pub tenant_id: String,
    /// Human-readable label for administrative UIs.
    pub name: String,
    /// Permission tags attached to the key. The first entry is used as the
    /// role when building an [`AuthContext`].
    pub permissions: Vec<String>,
    /// Timestamp when the key was created.
    pub created_at: DateTime<Utc>,
    /// Optional hard expiry. `None` means the key never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// If `true` the key is treated as invalid regardless of expiry or hash match.
    pub revoked: bool,
}

/// Stateless validator for raw API key strings against stored records.
///
/// Holds no fields; all methods are pure functions.
pub struct ApiKeyValidator;

impl ApiKeyValidator {
    /// Validate a raw API key against a list of known records.
    ///
    /// Returns the matching record only if:
    /// - the key is not revoked,
    /// - the SHA-256 hash matches,
    /// - and the current time is before any optional expiry.
    pub fn validate(key: &str, valid_keys: &[ApiKeyRecord]) -> Option<ApiKeyRecord> {
        let hash = Self::hash_key(key);
        let now = Utc::now();
        valid_keys
            .iter()
            .find(|record| {
                !record.revoked
                    && record.key_hash == hash
                    && record.expires_at.map(|exp| exp > now).unwrap_or(true)
            })
            .cloned()
    }

    /// Hash an API key using SHA-256, returning a lower-case hex string.
    pub fn hash_key(key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Generate a cryptographically random API key.
    ///
    /// Format: `"clz_"` prefix followed by 48 URL-safe base64 characters.
    /// Total length is 52 characters.
    ///
    /// # Entropy sources
    /// We concatenate three UUID v4 byte arrays and the current subsecond
    /// nanoseconds to build the random payload. This avoids adding `rand` as a
    /// direct dependency while still producing unpredictable keys.
    pub fn generate_key() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        // Use a mix of sources for entropy since we don't have `rand` here.
        // We combine UUID-style randomness via uuid crate (already a dep).
        let part1 = uuid::Uuid::new_v4().as_bytes().to_vec();
        let part2 = uuid::Uuid::new_v4().as_bytes().to_vec();
        let part3 = uuid::Uuid::new_v4().as_bytes().to_vec();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            .to_le_bytes();

        // 16 + 16 + 16 + 4 = 52 bytes of input entropy.
        let mut raw = Vec::with_capacity(52);
        raw.extend_from_slice(&part1);
        raw.extend_from_slice(&part2);
        raw.extend_from_slice(&part3);
        raw.extend_from_slice(&nanos);

        let encoded = URL_SAFE_NO_PAD.encode(&raw);
        // Truncate to 48 characters so the final key is exactly 52 chars long.
        let key_body: String = encoded.chars().take(48).collect();
        format!("clz_{key_body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_key_format() {
        let key = ApiKeyValidator::generate_key();
        assert!(key.starts_with("clz_"));
        assert_eq!(key.len(), 52); // "clz_" + 48 chars
    }

    #[test]
    fn test_generate_keys_are_unique() {
        let k1 = ApiKeyValidator::generate_key();
        let k2 = ApiKeyValidator::generate_key();
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_validate_valid_key() {
        let raw = ApiKeyValidator::generate_key();
        let record = ApiKeyRecord {
            id: "rec1".into(),
            key_hash: ApiKeyValidator::hash_key(&raw),
            user_id: "user1".into(),
            tenant_id: "default".into(),
            name: "test key".into(),
            permissions: vec!["read".into()],
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
        };
        let result = ApiKeyValidator::validate(&raw, &[record]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().user_id, "user1");
    }

    #[test]
    fn test_validate_revoked_key() {
        let raw = ApiKeyValidator::generate_key();
        let record = ApiKeyRecord {
            id: "rec2".into(),
            key_hash: ApiKeyValidator::hash_key(&raw),
            user_id: "user2".into(),
            tenant_id: "default".into(),
            name: "revoked key".into(),
            permissions: vec![],
            created_at: Utc::now(),
            expires_at: None,
            revoked: true,
        };
        let result = ApiKeyValidator::validate(&raw, &[record]);
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_expired_key() {
        use chrono::Duration;
        let raw = ApiKeyValidator::generate_key();
        let record = ApiKeyRecord {
            id: "rec3".into(),
            key_hash: ApiKeyValidator::hash_key(&raw),
            user_id: "user3".into(),
            tenant_id: "default".into(),
            name: "expired key".into(),
            permissions: vec![],
            created_at: Utc::now() - Duration::hours(2),
            expires_at: Some(Utc::now() - Duration::hours(1)),
            revoked: false,
        };
        let result = ApiKeyValidator::validate(&raw, &[record]);
        assert!(result.is_none());
    }
}
