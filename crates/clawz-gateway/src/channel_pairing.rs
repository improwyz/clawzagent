//! DM / peer pairing allowlist for inbound channels (standalone file store).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Pending pairing code awaiting user approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPair {
    pub channel_id: String,
    pub expires_at: DateTime<Utc>,
}

/// Approved peer for a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedPeer {
    pub channel_id: String,
    pub peer_id: String,
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PairingFile {
    #[serde(default)]
    pending: HashMap<String, PendingPair>,
    #[serde(default)]
    allowed: Vec<AllowedPeer>,
}

/// File-backed pairing store (`~/.clawz/channel-pairing.json`).
#[derive(Clone)]
pub struct PairingStore {
    path: PathBuf,
    inner: Arc<RwLock<PairingFile>>,
}

impl PairingStore {
    pub fn global() -> &'static PairingStore {
        static STORE: std::sync::OnceLock<PairingStore> = std::sync::OnceLock::new();
        STORE.get_or_init(PairingStore::open_default)
    }

    pub fn open_default() -> Self {
        let path = std::env::var("CLAWZ_PAIRING_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("CLAWZ_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        std::env::var("HOME")
                            .map(PathBuf::from)
                            .unwrap_or_else(|_| PathBuf::from("."))
                            .join(".clawz")
                    });
                home.join("channel-pairing.json")
            });
        let file = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Arc::new(RwLock::new(file)),
        }
    }

    async fn persist(&self) {
        let snapshot = {
            let guard = self.inner.read().await;
            guard.clone()
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    fn pairing_required(channel_config: &serde_json::Value) -> bool {
        channel_config
            .get("require_pairing")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| std::env::var("CLAWZ_CHANNEL_PAIRING").is_ok())
    }

    /// Whether inbound messages from `peer_id` are allowed for this channel.
    pub async fn is_allowed(
        &self,
        channel_id: &str,
        peer_id: &str,
        channel_config: &serde_json::Value,
    ) -> bool {
        if !Self::pairing_required(channel_config) {
            return true;
        }

        let file = self.inner.read().await;
        file.allowed
            .iter()
            .any(|p| p.channel_id == channel_id && p.peer_id == peer_id)
    }

    /// Create a short-lived pairing code (15 minutes).
    pub async fn create_code(&self, channel_id: &str) -> String {
        let code: String = uuid::Uuid::new_v4().to_string()[..8].to_uppercase();
        let expires_at = Utc::now() + Duration::minutes(15);
        {
            let mut file = self.inner.write().await;
            file.pending.insert(
                code.clone(),
                PendingPair {
                    channel_id: channel_id.to_string(),
                    expires_at,
                },
            );
        }
        self.persist().await;
        code
    }

    /// Approve a pending code for a specific peer id.
    pub async fn approve(&self, code: &str, peer_id: &str) -> Result<String, String> {
        let channel_id = {
            let mut file = self.inner.write().await;
            let pending = file
                .pending
                .remove(&code.to_uppercase())
                .ok_or_else(|| "invalid or expired pairing code".to_string())?;
            if pending.expires_at < Utc::now() {
                return Err("pairing code expired".into());
            }
            file.allowed.push(AllowedPeer {
                channel_id: pending.channel_id.clone(),
                peer_id: peer_id.to_string(),
                approved_at: Utc::now(),
            });
            pending.channel_id
        };
        self.persist().await;
        Ok(channel_id)
    }

    pub async fn list_allowed(&self, channel_id: Option<&str>) -> Vec<AllowedPeer> {
        let file = self.inner.read().await;
        file.allowed
            .iter()
            .filter(|p| channel_id.is_none_or(|id| id == p.channel_id))
            .cloned()
            .collect()
    }

    pub async fn revoke(&self, channel_id: &str, peer_id: &str) -> bool {
        let mut file = self.inner.write().await;
        let before = file.allowed.len();
        file.allowed
            .retain(|p| !(p.channel_id == channel_id && p.peer_id == peer_id));
        let removed = file.allowed.len() < before;
        drop(file);
        if removed {
            self.persist().await;
        }
        removed
    }
}
