//! Per-tenant user profiles stored on disk.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clawz_core::error::{ClawzError, Result};
use serde::{Deserialize, Serialize};

/// Operator / tenant preferences persisted across sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub tenant_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub preferences: HashMap<String, String>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub session_count: u64,
}

impl UserProfile {
    pub fn new(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            display_name: None,
            preferences: HashMap::new(),
            updated_at: Utc::now(),
            session_count: 0,
        }
    }

    pub fn touch_session(&mut self) {
        self.session_count += 1;
        self.updated_at = Utc::now();
    }
}

/// File-backed profile store at `{root}/{tenant_id}.json`.
pub struct UserProfileStore {
    root: PathBuf,
}

impl UserProfileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_home() -> Self {
        let home = std::env::var("CLAWZ_HOME").map(PathBuf::from).unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp/clawz"))
                .join(".clawz")
        });
        Self::new(home.join("profiles"))
    }

    fn profile_path(&self, tenant_id: &str) -> PathBuf {
        self.root.join(format!("{}.json", sanitize_tenant(tenant_id)))
    }

    pub async fn load(&self, tenant_id: &str) -> Result<UserProfile> {
        let path = self.profile_path(tenant_id);
        if !path.exists() {
            return Ok(UserProfile::new(tenant_id));
        }
        let data = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ClawzError::Internal(format!("read profile: {e}")))?;
        serde_json::from_str(&data)
            .map_err(|e| ClawzError::Serialization(format!("profile json: {e}")))
    }

    pub async fn save(&self, profile: &UserProfile) -> Result<()> {
        if let Some(parent) = self.root.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ClawzError::Internal(format!("create profile parent: {e}")))?;
        }
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| ClawzError::Internal(format!("create profiles dir: {e}")))?;
        let path = self.profile_path(&profile.tenant_id);
        let body = serde_json::to_string_pretty(profile)
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;
        tokio::fs::write(&path, body)
            .await
            .map_err(|e| ClawzError::Internal(format!("write profile: {e}")))?;
        Ok(())
    }

    pub async fn touch_session(&self, tenant_id: &str) -> Result<UserProfile> {
        let mut profile = self.load(tenant_id).await?;
        profile.touch_session();
        self.save(&profile).await?;
        Ok(profile)
    }
}

fn sanitize_tenant(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn profile_roundtrip() {
        let dir = std::env::temp_dir().join(format!("clawz-prof-{}", uuid::Uuid::new_v4()));
        let store = UserProfileStore::new(&dir);
        let mut p = store.load("tenant-a").await.unwrap();
        p.display_name = Some("Alice".into());
        p.preferences.insert("theme".into(), "dark".into());
        store.save(&p).await.unwrap();
        let loaded = store.load("tenant-a").await.unwrap();
        assert_eq!(loaded.display_name.as_deref(), Some("Alice"));
        assert_eq!(loaded.preferences.get("theme").map(String::as_str), Some("dark"));
    }
}
