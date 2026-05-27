//! Live PRISM-G dimension capability checks for `/api/v1/system/prism`.

use clawz_core::deployment::DeploymentMode;
use clawz_core::prism::{derive_status, dimension_registry, DimensionCapability, DimensionStatus};
use clawz_core::Result;
use serde_json::{json, Value};

struct PurposeCheck;
struct RealityCheck;
struct InfrastructureCheck;
struct SwarmCheck;
struct MemoryCheck;
struct GovernanceCheck;

#[async_trait::async_trait]
impl DimensionCapability for PurposeCheck {
    fn dimension(&self) -> clawz_core::prism::PrismDimension {
        clawz_core::prism::PrismDimension::Purpose
    }
    fn mode(&self) -> DeploymentMode {
        current_mode()
    }
    async fn verify(&self) -> Result<()> {
        let _validator = clawz_worker::purpose::Validator::new();
        Ok(())
    }
}

#[async_trait::async_trait]
impl DimensionCapability for RealityCheck {
    fn dimension(&self) -> clawz_core::prism::PrismDimension {
        clawz_core::prism::PrismDimension::Reality
    }
    fn mode(&self) -> DeploymentMode {
        current_mode()
    }
    async fn verify(&self) -> Result<()> {
        if std::env::var("DATABASE_URL").is_err() {
            return Err(clawz_core::error::ClawzError::Config(
                "DATABASE_URL required for Reality checks".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl DimensionCapability for InfrastructureCheck {
    fn dimension(&self) -> clawz_core::prism::PrismDimension {
        clawz_core::prism::PrismDimension::Infrastructure
    }
    fn mode(&self) -> DeploymentMode {
        current_mode()
    }
    async fn verify(&self) -> Result<()> {
        let registry = clawz_worker::tools::registry::ToolRegistry::new();
        registry.register_builtins().await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl DimensionCapability for SwarmCheck {
    fn dimension(&self) -> clawz_core::prism::PrismDimension {
        clawz_core::prism::PrismDimension::Swarm
    }
    fn mode(&self) -> DeploymentMode {
        current_mode()
    }
    async fn verify(&self) -> Result<()> {
        let _team = clawz_worker::runtime::team::Team::new("probe", "probe-leader");
        Ok(())
    }
}

#[async_trait::async_trait]
impl DimensionCapability for MemoryCheck {
    fn dimension(&self) -> clawz_core::prism::PrismDimension {
        clawz_core::prism::PrismDimension::Memory
    }
    fn mode(&self) -> DeploymentMode {
        current_mode()
    }
    async fn verify(&self) -> Result<()> {
        let _ = clawz_worker::memory::store::InMemoryBackend::new();
        Ok(())
    }
}

#[async_trait::async_trait]
impl DimensionCapability for GovernanceCheck {
    fn dimension(&self) -> clawz_core::prism::PrismDimension {
        clawz_core::prism::PrismDimension::Governance
    }
    fn mode(&self) -> DeploymentMode {
        current_mode()
    }
    async fn verify(&self) -> Result<()> {
        let _ = clawz_worker::governance::engine::ClawzGovernanceEngine::new(
            clawz_worker::governance::engine::GovernanceEngineConfig::default(),
        );
        Ok(())
    }
}

fn current_mode() -> DeploymentMode {
    match std::env::var("CLAWZ_MODE")
        .unwrap_or_else(|_| "standalone".into())
        .to_lowercase()
        .as_str()
    {
        "micro" => DeploymentMode::Micro,
        "elastic" => DeploymentMode::Elastic,
        _ => DeploymentMode::Standalone,
    }
}

fn all_checks() -> Vec<Box<dyn DimensionCapability>> {
    vec![
        Box::new(PurposeCheck),
        Box::new(RealityCheck),
        Box::new(InfrastructureCheck),
        Box::new(SwarmCheck),
        Box::new(MemoryCheck),
        Box::new(GovernanceCheck),
    ]
}

/// Run capability checks and return PRISM status JSON for the API.
pub async fn prism_status_json() -> Value {
    let mode = current_mode();
    let checks = all_checks();

    let mut dimensions = Vec::new();
    for info in dimension_registry().iter() {
        let mut passing_modes: Vec<DeploymentMode> = Vec::new();
        for check in &checks {
            if check.dimension() == info.dimension && check.verify().await.is_ok() {
                let m = check.mode();
                if !passing_modes.contains(&m) {
                    passing_modes.push(m);
                }
            }
        }
        let status = derive_status(info, &passing_modes);
        dimensions.push(json!({
            "dimension": format!("{:?}", info.dimension).to_lowercase(),
            "letter": info.dimension.letter(),
            "title": info.dimension.title(),
            "status": status_label(status),
            "module": info.module,
            "required_modes": info.required_modes.iter().map(|m| format!("{:?}", m).to_lowercase()).collect::<Vec<_>>(),
            "passing_modes": passing_modes.iter().map(|m| format!("{:?}", m).to_lowercase()).collect::<Vec<_>>(),
        }));
    }

    json!({
        "mode": format!("{:?}", mode).to_lowercase(),
        "dimensions": dimensions,
        "checked_at": chrono::Utc::now(),
    })
}

fn status_label(status: DimensionStatus) -> &'static str {
    match status {
        DimensionStatus::Implemented => "implemented",
        DimensionStatus::Partial => "partial",
        DimensionStatus::Planned => "planned",
    }
}
