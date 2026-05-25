//! Persistence layer and shared helpers for deployment state.
//!
//! `DeploymentStore` abstracts where deployment records live (in-memory, database,
//! object storage, etc.).  The default `MemoryDeploymentStore` is suitable for
//! single-node gateway instances; production deployments can swap in a persistent
//! implementation.
//!
//! Helper functions in this module convert high-level structures (e.g. env-var maps)
//! into provider-specific CLI flags.
//!
//! ## Key dependencies
//!
//! * `crate::deploy::provider` — `DeploymentInfo` and `DeploymentStatus` types stored here.
//! * `clawz_core::error` — `ClawzError` variants for poisoned locks and missing records.

// Dependency: provider types define the record shape we persist.
use crate::deploy::provider::{DeploymentInfo, DeploymentStatus};
use clawz_core::error::{ClawzError, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Trait for persisting and retrieving deployment metadata.
///
/// Implementations must be `Send + Sync` because they are held inside
/// `Arc<dyn DeploymentStore>` and accessed across async boundaries.
pub trait DeploymentStore: Send + Sync {
    /// Persist a new or updated deployment record.
    fn save(&self, info: DeploymentInfo) -> Result<()>;
    /// Fetch a single deployment by its unique ID, returning `None` if absent.
    fn get(&self, id: &str) -> Result<Option<DeploymentInfo>>;
    /// Return every deployment currently tracked.
    fn list(&self) -> Result<Vec<DeploymentInfo>>;
    /// Update only the status field of an existing deployment.
    fn update_status(&self, id: &str, status: DeploymentStatus) -> Result<()>;
    /// Remove a deployment record from the store.
    fn remove(&self, id: &str) -> Result<()>;
}

/// In-memory `DeploymentStore` backed by a `Mutex<HashMap>`.
///
/// All data is lost when the process exits — use this for tests or
/// ephemeral gateway nodes.  For durability, replace with a database-backed impl.
#[derive(Debug, Default)]
pub struct MemoryDeploymentStore {
    /// Thread-safe map from deployment ID → deployment record.
    deployments: Arc<Mutex<HashMap<String, DeploymentInfo>>>,
}

impl MemoryDeploymentStore {
    /// Create a fresh, empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl DeploymentStore for MemoryDeploymentStore {
    fn save(&self, info: DeploymentInfo) -> Result<()> {
        let mut map = self.deployments.lock().map_err(|e| {
            // Mutex is poisoned when a panicking thread held the lock.
            ClawzError::Internal(format!("deployment store lock poisoned: {e}"))
        })?;
        map.insert(info.id.clone(), info);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<DeploymentInfo>> {
        let map = self.deployments.lock().map_err(|e| {
            ClawzError::Internal(format!("deployment store lock poisoned: {e}"))
        })?;
        Ok(map.get(id).cloned())
    }

    fn list(&self) -> Result<Vec<DeploymentInfo>> {
        let map = self.deployments.lock().map_err(|e| {
            ClawzError::Internal(format!("deployment store lock poisoned: {e}"))
        })?;
        Ok(map.values().cloned().collect())
    }

    fn update_status(&self, id: &str, status: DeploymentStatus) -> Result<()> {
        let mut map = self.deployments.lock().map_err(|e| {
            ClawzError::Internal(format!("deployment store lock poisoned: {e}"))
        })?;
        if let Some(info) = map.get_mut(id) {
            info.status = status;
            Ok(())
        } else {
            Err(ClawzError::NotFound {
                entity: "deployment".into(),
                id: id.into(),
            })
        }
    }

    fn remove(&self, id: &str) -> Result<()> {
        let mut map = self.deployments.lock().map_err(|e| {
            ClawzError::Internal(format!("deployment store lock poisoned: {e}"))
        })?;
        map.remove(id);
        Ok(())
    }
}

/// Generate a unique deployment ID with the given prefix.
///
/// Format: `{prefix}-{uuid-v4}`.  The prefix helps humans identify the target
/// provider when reading raw IDs.
pub fn generate_deployment_id(prefix: &str) -> String {
    format!("{}-{}", prefix, uuid::Uuid::new_v4())
}

/// Convert a map of environment variables into Docker `-e` CLI flags.
///
/// Each key-value pair yields two entries: `["-e", "KEY=VALUE"]`.  The resulting
/// vector can be passed directly to a `docker run` invocation.
pub fn env_vars_to_docker_flags(env_vars: &std::collections::HashMap<String, String>) -> Vec<String> {
    env_vars
        .iter()
        .flat_map(|(k, v)| vec!["-e".to_string(), format!("{}={}", k, v)])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_store_save_and_get() {
        let store = MemoryDeploymentStore::new();
        let info = DeploymentInfo {
            id: "dep-1".into(),
            url: "https://example.com".into(),
            status: DeploymentStatus::Running,
        };
        store.save(info.clone()).unwrap();
        let fetched = store.get("dep-1").unwrap().unwrap();
        assert_eq!(fetched.id, "dep-1");
        assert_eq!(fetched.url, "https://example.com");
    }

    #[test]
    fn test_memory_store_update_status() {
        let store = MemoryDeploymentStore::new();
        let info = DeploymentInfo {
            id: "dep-2".into(),
            url: "https://example.com".into(),
            status: DeploymentStatus::Pending,
        };
        store.save(info).unwrap();
        store.update_status("dep-2", DeploymentStatus::Running).unwrap();
        let fetched = store.get("dep-2").unwrap().unwrap();
        assert_eq!(fetched.status, DeploymentStatus::Running);
    }

    #[test]
    fn test_generate_deployment_id() {
        let id = generate_deployment_id("gcr");
        assert!(id.starts_with("gcr-"));
    }

    #[test]
    fn test_env_vars_to_docker_flags() {
        let mut env = std::collections::HashMap::new();
        env.insert("KEY1".into(), "value1".into());
        env.insert("KEY2".into(), "value2".into());
        let flags = env_vars_to_docker_flags(&env);
        assert!(flags.contains(&"-e".to_string()));
        assert!(flags.contains(&"KEY1=value1".to_string()));
    }
}
