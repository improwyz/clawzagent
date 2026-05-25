//! Deployment orchestration module for the ClawZ Gateway.
//!
//! This module implements the `DeployManager` registry which holds 18 provider
//! adapters and auto-selects the best match based on `DeployMode`.  It is the
//! central dispatch point for all cloud-deployment operations initiated by the
//! HTTP gateway.
//!
//! ## Architecture
//!
//! * **Provider trait** (`provider::DeployProvider`) — uniform interface that every
//!   cloud/edge adapter implements.
//! * **Common utilities** (`common`) — `DeploymentStore` abstractions and helpers
//!   for generating deployment IDs and converting env-vars to Docker flags.
//! * **18 adapters** — one submodule per target (Fly.io, Railway, AWS Lambda,
//!   Vercel, Kubernetes, etc.).
//! * **Auto-selection** — `DeployManager::auto_select_provider` maps `DeployMode`
//!   to a ranked list of candidate providers.
//!
//! ## Provider tiers
//!
//! 1. Container platforms — Fly.io, Railway, Hetzner, Northflank, Sliplane
//! 2. Cloud managed / serverless — Google Cloud Run, Oracle Cloud, AWS Lambda,
//!    Azure Functions, MassiveGrid
//! 3. Edge / serverless — Cloudflare, Fastly, Vercel
//! 4. Infrastructure — Kubernetes
//!
//! ## Key dependencies
//!
//! * `clawz_core::error` — unified error types (`ClawzError`, `Result`).
//! * `provider` — core trait and configuration structs re-exported here.
//! * `common` — persistence trait used by `DeployManager` to track deployment state.

pub mod aws_lambda;
pub mod azure_functions;
pub mod cloudflare;
pub mod common;
pub mod fastly;
pub mod fly_io;
pub mod google_cloud_run;
pub mod hetzner;
pub mod kubernetes;
pub mod massivegrid;
pub mod northflank;
pub mod oracle_cloud;
pub mod provider;
pub mod railway;
pub mod sliplane;
pub mod tofu;
pub mod vercel;

pub use aws_lambda::AwsLambdaAdapter;
pub use azure_functions::AzureFunctionsAdapter;
pub use cloudflare::CloudflareAdapter;
pub use common::{DeploymentStore, MemoryDeploymentStore};
pub use fastly::FastlyAdapter;
pub use fly_io::FlyIoAdapter;
pub use google_cloud_run::GoogleCloudRunAdapter;
pub use hetzner::HetznerAdapter;
pub use kubernetes::KubernetesAdapter;
pub use massivegrid::MassiveGridAdapter;
pub use northflank::NorthflankAdapter;
pub use oracle_cloud::OracleCloudAdapter;
pub use provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus,
    ProviderCredentials,
};
pub use railway::RailwayAdapter;
pub use sliplane::SliplaneAdapter;
pub use tofu::TofuRunner;
pub use vercel::VercelAdapter;

// Dependency: clawz_core::error for unified error handling across all provider calls.
use clawz_core::error::{ClawzError, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// Lightweight metadata about a registered provider, suitable for UI listings.
///
/// Does **not** hold credentials — those are supplied at operation time.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// Canonical provider identifier, e.g. `"fly_io"` or `"aws_lambda"`.
    pub id: String,
    /// Human-readable label shown in dashboards, e.g. `"Fly.io"`.
    pub display_name: String,
    /// Deployment-mode strings this provider advertises (`"docker"`, `"native"`, `"wasm"`).
    pub deploy_modes: Vec<&'static str>,
}

/// `DeployManager` owns a set of named provider adapters and orchestrates
/// selection, validation, deployment, and status tracking.
pub struct DeployManager {
    /// Map from provider ID → shared provider instance.
    providers: HashMap<String, Arc<dyn DeployProvider>>,
    /// Persistent (or in-memory) store for deployment records.
    store: Arc<dyn DeploymentStore>,
}

impl DeployManager {
    /// Create a `DeployManager` pre-populated with all supported providers.
    ///
    /// Callers that need non-default constructor arguments (account IDs, regions, etc.)
    /// should use `with_providers` instead.
    pub fn new_default(store: Arc<dyn DeploymentStore>) -> Self {
        let mut m = Self {
            providers: HashMap::new(),
            store,
        };

        // Tier 1 — container platforms
        m.register(Arc::new(FlyIoAdapter::new("personal")));
        m.register(Arc::new(RailwayAdapter::new()));
        m.register(Arc::new(HetznerAdapter::new()));
        m.register(Arc::new(NorthflankAdapter::new("default")));
        m.register(Arc::new(SliplaneAdapter::new()));

        // Tier 2 — cloud managed container / serverless
        m.register(Arc::new(GoogleCloudRunAdapter::new("default-project", "us-central1")));
        m.register(Arc::new(OracleCloudAdapter::new("default-tenancy", "us-ashburn-1")));
        m.register(Arc::new(AwsLambdaAdapter::new("us-east-1", "000000000000")));
        m.register(Arc::new(AzureFunctionsAdapter::new("", "default")));
        m.register(Arc::new(MassiveGridAdapter::new("https://app.massivegrid.com")));

        // Tier 3 — edge / serverless
        m.register(Arc::new(CloudflareAdapter::new("default")));
        m.register(Arc::new(FastlyAdapter::new()));
        m.register(Arc::new(VercelAdapter::new(None::<String>)));

        // Tier 4 — infra
        m.register(Arc::new(KubernetesAdapter::new(
            "https://kubernetes.default.svc",
            "default",
        )));

        m
    }

    /// Create a `DeployManager` with an explicit set of providers.
    pub fn with_providers(
        providers: Vec<Arc<dyn DeployProvider>>,
        store: Arc<dyn DeploymentStore>,
    ) -> Self {
        let mut m = Self {
            providers: HashMap::new(),
            store,
        };
        for p in providers {
            m.register(p);
        }
        m
    }

    /// Insert a provider into the internal HashMap keyed by its `provider_id`.
    fn register(&mut self, provider: Arc<dyn DeployProvider>) {
        self.providers.insert(provider.provider_id().to_string(), provider);
    }

    // ── Provider discovery ─────────────────────────────────────────────────

    /// Return a list of all registered provider IDs.
    pub fn list_providers(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.providers.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }

    /// Return metadata for every registered provider, sorted by ID.
    pub fn provider_infos(&self) -> Vec<ProviderInfo> {
        let mut infos: Vec<ProviderInfo> = self
            .providers
            .values()
            .map(|p| {
                // Map each DeployMode variant to its canonical string representation.
                let modes: Vec<&'static str> = p
                    .supported_modes()
                    .iter()
                    .map(|m| match m {
                        DeployMode::Docker { .. } => "docker",
                        DeployMode::NativeBinary => "native",
                        DeployMode::Wasm => "wasm",
                    })
                    .collect();
                ProviderInfo {
                    id: p.provider_id().to_string(),
                    display_name: p.display_name().to_string(),
                    deploy_modes: modes,
                }
            })
            .collect();
        infos.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    // ── Core operations ────────────────────────────────────────────────────

    /// Retrieve a provider by its ID string.
    pub fn get_provider(&self, provider_id: &str) -> Result<&Arc<dyn DeployProvider>> {
        self.providers.get(provider_id).ok_or_else(|| ClawzError::NotFound {
            entity: "deploy_provider".into(),
            id: provider_id.into(),
        })
    }

    /// Validate credentials for the named provider.
    pub async fn validate_config(
        &self,
        provider_id: &str,
        creds: &ProviderCredentials,
    ) -> Result<()> {
        let provider = self.get_provider(provider_id)?;
        provider.validate_credentials(creds).await
    }

    /// Deploy to the named provider; persist the resulting `DeploymentInfo`.
    pub async fn deploy(
        &self,
        provider_id: &str,
        config: &DeployConfig,
    ) -> Result<DeploymentInfo> {
        let provider = self.get_provider(provider_id)?;
        let info = provider.deploy(config).await?;
        self.store.save(info.clone())?;
        Ok(info)
    }

    /// Query deployment status from live provider and update the local store.
    pub async fn deployment_status(&self, provider_id: &str, deployment_id: &str) -> Result<DeploymentStatus> {
        let provider = self.get_provider(provider_id)?;
        let status = provider.status(deployment_id).await?;
        self.store.update_status(deployment_id, status)?;
        Ok(status)
    }

    /// Stop / tear down a deployment.
    pub async fn destroy(&self, provider_id: &str, deployment_id: &str) -> Result<()> {
        let provider = self.get_provider(provider_id)?;
        provider.destroy(deployment_id).await?;
        self.store.update_status(deployment_id, DeploymentStatus::Stopped)?;
        Ok(())
    }

    /// List all deployments tracked in the local store.
    pub fn list_deployments(&self) -> Result<Vec<DeploymentInfo>> {
        self.store.list()
    }

    /// Choose a provider based on the deployment mode in the config.
    ///
    /// - `Wasm` → prefer Fastly, then Cloudflare
    /// - `Docker` → prefer Fly.io, then Railway
    /// - `NativeBinary` → prefer AWS Lambda, then Vercel
    pub fn auto_select_provider(&self, config: &DeployConfig) -> Result<&str> {
        let candidates: &[&str] = match &config.mode {
            DeployMode::Wasm => &["fastly", "cloudflare"],
            DeployMode::Docker { .. } => &["fly_io", "railway", "northflank", "sliplane"],
            DeployMode::NativeBinary => &["aws_lambda", "vercel", "fly_io"],
        };
        for &id in candidates {
            if self.providers.contains_key(id) {
                return Ok(id);
            }
        }
        Err(ClawzError::Validation(format!(
            "No suitable provider found for deploy mode {:?}",
            config.mode
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager() -> DeployManager {
        let store = Arc::new(MemoryDeploymentStore::new());
        DeployManager::new_default(store)
    }

    #[test]
    fn test_list_providers_sorted() {
        let m = make_manager();
        let ids = m.list_providers();
        assert!(ids.contains(&"fly_io"));
        assert!(ids.contains(&"aws_lambda"));
        assert!(ids.contains(&"fastly"));
        assert!(ids.contains(&"vercel"));
        assert!(ids.contains(&"kubernetes"));
        assert!(ids.contains(&"northflank"));
        assert!(ids.contains(&"sliplane"));
        assert!(ids.contains(&"massivegrid"));
        assert!(ids.contains(&"azure_functions"));
        // Verify sorted order
        let sorted = {
            let mut v = ids.clone();
            v.sort_unstable();
            v
        };
        assert_eq!(ids, sorted);
    }

    #[test]
    fn test_get_provider_unknown() {
        let m = make_manager();
        assert!(m.get_provider("nonexistent").is_err());
    }

    #[test]
    fn test_auto_select_wasm() {
        let m = make_manager();
        let config = DeployConfig {
            mode: DeployMode::Wasm,
            env_vars: Default::default(),
            region: None,
            replicas: 1,
        };
        let id = m.auto_select_provider(&config).unwrap();
        assert_eq!(id, "fastly");
    }

    #[test]
    fn test_auto_select_docker() {
        let m = make_manager();
        let config = DeployConfig {
            mode: DeployMode::Docker { image: "nginx".into() },
            env_vars: Default::default(),
            region: None,
            replicas: 1,
        };
        let id = m.auto_select_provider(&config).unwrap();
        assert_eq!(id, "fly_io");
    }

    #[test]
    fn test_auto_select_native() {
        let m = make_manager();
        let config = DeployConfig {
            mode: DeployMode::NativeBinary,
            env_vars: Default::default(),
            region: None,
            replicas: 1,
        };
        let id = m.auto_select_provider(&config).unwrap();
        assert_eq!(id, "aws_lambda");
    }

    #[test]
    fn test_provider_infos_all_present() {
        let m = make_manager();
        let infos = m.provider_infos();
        let ids: Vec<&str> = infos.iter().map(|i| i.id.as_str()).collect();
        assert!(ids.contains(&"fastly"));
        assert!(ids.contains(&"vercel"));
        assert!(ids.contains(&"kubernetes"));
    }
}
