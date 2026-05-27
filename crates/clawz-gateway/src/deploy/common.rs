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

/// External app/script/service name derived from a gateway deployment id.
pub fn external_resource_name(deployment_id: &str) -> String {
    format!("clawz-{}", deployment_id.replace('-', ""))
}

/// Kubernetes Deployment/Service name (short suffix for DNS limits).
pub fn k8s_resource_name(deployment_id: &str) -> String {
    if deployment_id.len() >= 12 {
        format!("clawz-{}", &deployment_id[4..12])
    } else {
        external_resource_name(deployment_id)
    }
}

/// Fastly service name suffix (`clawz-{8}`) from `fastly-{uuid}` ids.
pub fn fastly_service_key(deployment_id: &str) -> Option<String> {
    if deployment_id.starts_with("fastly-") && deployment_id.len() >= 15 {
        Some(format!("clawz-{}", &deployment_id[7..15]))
    } else {
        None
    }
}

fn id_suffix(deployment_id: &str, start: usize, len: usize) -> String {
    if deployment_id.len() >= start + len {
        deployment_id[start..start + len].to_string()
    } else {
        deployment_id.get(start..).unwrap_or("").to_string()
    }
}

/// AWS Lambda function name from a `lambda-{uuid}` deployment id.
pub fn lambda_function_name(deployment_id: &str) -> String {
    format!("clawz-{}", id_suffix(deployment_id, 7, 8))
}

/// Vercel project name from a `vcl-{uuid}` deployment id.
pub fn vercel_project_name(deployment_id: &str) -> String {
    format!("clawz-{}", id_suffix(deployment_id, 4, 8))
}

/// Azure Function App name from an `az-{uuid}` deployment id.
pub fn azure_app_name(deployment_id: &str) -> String {
    format!("clawz{}", id_suffix(deployment_id, 3, 8).replace('-', ""))
}

/// Northflank / Sliplane service name from `nf-` / `sp-` ids.
pub fn short_service_name(deployment_id: &str) -> String {
    format!("clawz-{}", id_suffix(deployment_id, 3, 8))
}

/// MassiveGrid Jelastic environment name from `mg-{uuid}` ids.
pub fn massivegrid_env_name(deployment_id: &str) -> String {
    format!("clawz-{}", id_suffix(deployment_id, 3, 8))
}

/// Returns true when an HTTP status indicates a successful destroy (including 404).
pub fn destroy_http_ok(status: reqwest::StatusCode) -> bool {
    status.is_success() || status.as_u16() == 404
}

/// Parse `DeploymentStatus` from the `deployments.status` column.
pub fn deployment_status_from_str(s: &str) -> DeploymentStatus {
    match s {
        "running" => DeploymentStatus::Running,
        "stopped" => DeploymentStatus::Stopped,
        "failed" => DeploymentStatus::Failed,
        _ => DeploymentStatus::Pending,
    }
}

pub fn deployment_status_to_str(status: DeploymentStatus) -> &'static str {
    match status {
        DeploymentStatus::Pending => "pending",
        DeploymentStatus::Running => "running",
        DeploymentStatus::Stopped => "stopped",
        DeploymentStatus::Failed => "failed",
    }
}

/// Stable UUID for the `deployments` table from a gateway deployment id.
pub fn deployment_db_uuid(deployment_id: &str) -> uuid::Uuid {
    if let Some((_prefix, rest)) = deployment_id.split_once('-') {
        if let Ok(u) = uuid::Uuid::parse_str(rest) {
            return u;
        }
    }
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(deployment_id.as_bytes());
    let bytes: [u8; 16] = hash[..16]
        .try_into()
        .expect("sha256 produces 32 bytes");
    uuid::Uuid::from_bytes(bytes)
}

/// Resolve a bearer/API token from deploy credentials or an environment variable.
pub fn resolve_api_token(
    config: &crate::deploy::provider::DeployConfig,
    env_var: &str,
    label: &str,
) -> clawz_core::Result<String> {
    use clawz_core::error::ClawzError;
    if let Some(ref creds) = config.credentials {
        if let Some(token) = creds.api_token.clone() {
            return Ok(token);
        }
    }
    std::env::var(env_var).map_err(|_| {
        ClawzError::Auth(format!(
            "{label} token required: set config.credentials.api_token or {env_var}"
        ))
    })
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
            provider_id: "fly_io".into(),
            external_resource: None,
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
            provider_id: "fly_io".into(),
            external_resource: None,
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
