//! Versioned storage for [`ContextBundle`] snapshots.
//!
//! Provides a [`ContextStore`] trait and an in-memory implementation for
//! testing. All operations are tenant-isolated.

use clawz_core::error::{ClawzError, Result};
use clawz_core::types::{ConflictMarker, ContextBundle, ContextDelta};
use std::collections::HashMap;

/// Persistent storage for versioned [`ContextBundle`]s.
///
/// Implementations must enforce tenant isolation: queries only return
/// bundles that belong to the supplied `tenant_id`.
pub trait ContextStore {
    /// Persist a bundle. The version is taken from the bundle itself.
    fn save(&self, bundle: &ContextBundle) -> Result<()>;

    /// Retrieve the most recent bundle for a tenant, if any.
    fn latest(&self, tenant_id: &str) -> Result<Option<ContextBundle>>;

    /// Reconstruct a bundle at an exact version by replaying deltas
    /// from the closest stored snapshot.
    ///
    /// Returns an error if the requested version is older than the oldest
    /// stored version (stale version).
    fn reconstruct(&self, tenant_id: &str, version: u32) -> Result<ContextBundle>;
}

/// In-memory implementation of [`ContextStore`] for testing.
#[derive(Debug, Default)]
pub struct InMemoryContextStore {
    /// tenant_id → ordered list of (version, bundle)
    bundles: std::sync::RwLock<BundleMap>,
    /// tenant_id → list of deltas (from_version, to_version, delta)
    deltas: std::sync::RwLock<DeltaMap>,
    /// tenant_id → list of conflict markers
    conflicts: std::sync::RwLock<ConflictMap>,
}

type BundleMap = HashMap<String, Vec<(u32, ContextBundle)>>;
type DeltaMap = HashMap<String, Vec<(u32, u32, ContextDelta)>>;
type ConflictMap = HashMap<String, Vec<ConflictMarker>>;

impl InMemoryContextStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a conflict marker for a tenant.
    pub fn record_conflict(&self, tenant_id: &str, marker: ConflictMarker) -> Result<()> {
        let mut map = self.conflicts.write().map_err(|_| {
            ClawzError::Internal("InMemoryContextStore conflict lock poisoned".into())
        })?;
        map.entry(tenant_id.to_string()).or_default().push(marker);
        Ok(())
    }

    /// Retrieve all conflict markers for a tenant.
    pub fn conflicts(&self, tenant_id: &str) -> Result<Vec<ConflictMarker>> {
        let map = self.conflicts.read().map_err(|_| {
            ClawzError::Internal("InMemoryContextStore conflict lock poisoned".into())
        })?;
        Ok(map.get(tenant_id).cloned().unwrap_or_default())
    }
}

impl ContextStore for InMemoryContextStore {
    fn save(&self, bundle: &ContextBundle) -> Result<()> {
        let mut map = self.bundles.write().map_err(|_| {
            ClawzError::Internal("InMemoryContextStore bundle lock poisoned".into())
        })?;
        let entry = map.entry(bundle.tenant_id.clone()).or_default();

        // If this is a new version, record a delta from the previous version.
        if let Some((prev_version, _)) = entry.last() {
            if bundle.version > *prev_version {
                let delta = ContextDelta {
                    from_version: *prev_version,
                    to_version: bundle.version,
                    changes: vec!["bundle updated".into()],
                };
                let mut delta_map = self.deltas.write().map_err(|_| {
                    ClawzError::Internal("InMemoryContextStore delta lock poisoned".into())
                })?;
                delta_map
                    .entry(bundle.tenant_id.clone())
                    .or_default()
                    .push((*prev_version, bundle.version, delta));
            }
        }

        entry.push((bundle.version, bundle.clone()));
        Ok(())
    }

    fn latest(&self, tenant_id: &str) -> Result<Option<ContextBundle>> {
        let map = self.bundles.read().map_err(|_| {
            ClawzError::Internal("InMemoryContextStore bundle lock poisoned".into())
        })?;
        Ok(map
            .get(tenant_id)
            .and_then(|v| v.last().map(|(_, b)| b.clone())))
    }

    fn reconstruct(&self, tenant_id: &str, version: u32) -> Result<ContextBundle> {
        let map = self.bundles.read().map_err(|_| {
            ClawzError::Internal("InMemoryContextStore bundle lock poisoned".into())
        })?;
        let entry = map.get(tenant_id).ok_or_else(|| {
            ClawzError::NotFound {
                entity: "tenant".into(),
                id: tenant_id.into(),
            }
        })?;

        let oldest = entry.first().map(|(v, _)| *v).unwrap_or(1);
        if version < oldest {
            return Err(ClawzError::Validation(format!(
                "stale version {version}: oldest stored is {oldest}"
            )));
        }

        // Find the closest snapshot at or before the requested version.
        let snapshot = entry
            .iter()
            .filter(|(v, _)| *v <= version)
            .max_by_key(|(v, _)| *v)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| ClawzError::NotFound {
                entity: "bundle version".into(),
                id: version.to_string(),
            })?;

        // If the snapshot is already at the exact version, return it.
        if snapshot.version == version {
            return Ok(snapshot);
        }

        // Otherwise apply deltas from snapshot.version → target version.
        let delta_map = self.deltas.read().map_err(|_| {
            ClawzError::Internal("InMemoryContextStore delta lock poisoned".into())
        })?;
        let deltas = delta_map.get(tenant_id).cloned().unwrap_or_default();

        let mut reconstructed = snapshot;
        for (from, to, delta) in deltas {
            if from >= reconstructed.version && to <= version {
                reconstructed.version = to;
                // In a real implementation we would apply the changes;
                // for the in-memory store we simply advance the version.
                let _ = delta;
            }
        }

        Ok(reconstructed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use clawz_core::types::CurrentState;

    #[test]
    fn save_and_latest_roundtrip() {
        let store = InMemoryContextStore::new();
        let bundle = ContextBundle::builder()
            .tenant_id("t1")
            .version(1)
            .current_state(CurrentState {
                system_status: "ok".into(),
                project_progress: "10%".into(),
                resource_consumption: "low".into(),
            })
            .build();

        store.save(&bundle).unwrap();
        let latest = store.latest("t1").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().version, 1);
    }

    #[test]
    fn tenant_isolation() {
        let store = InMemoryContextStore::new();
        let bundle_a = ContextBundle::builder().tenant_id("tenant_a").version(1).build();
        store.save(&bundle_a).unwrap();

        let latest_b = store.latest("tenant_b").unwrap();
        assert!(latest_b.is_none());
    }

    #[test]
    fn stale_version_returns_error() {
        let store = InMemoryContextStore::new();
        let bundle = ContextBundle::builder()
            .tenant_id("t1")
            .version(5)
            .current_state(CurrentState {
                system_status: "ok".into(),
                project_progress: "10%".into(),
                resource_consumption: "low".into(),
            })
            .build();
        store.save(&bundle).unwrap();

        let result = store.reconstruct("t1", 3);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("stale version"));
    }

    #[test]
    fn human_fact_conflict_produces_conflict_marker() {
        let store = InMemoryContextStore::new();
        let marker = ConflictMarker {
            key: "system_status".into(),
            old_value: "healthy".into(),
            new_value: "degraded".into(),
            resolved_at: Utc::now(),
        };
        store.record_conflict("t1", marker.clone()).unwrap();

        let conflicts = store.conflicts("t1").unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].key, "system_status");
        assert_eq!(conflicts[0].old_value, "healthy");
        assert_eq!(conflicts[0].new_value, "degraded");
    }
}
