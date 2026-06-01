//! Bootstrap helpers for gateway and worker binaries.

use std::sync::Arc;

use clawz_core::deployment::DeploymentMode;
use clawz_core::traits::AgentScheduler;
use clawz_services::Platform;
use clawz_services::execution::{ExecutionClient, HttpExecutionClient};
use clawz_worker::client::InProcessExecutionClient;
use clawz_worker::governance::approval::ApprovalWorkflow;
use clawz_worker::orchestration::factory::create_scheduler;
use clawz_worker::service::WorkerService;

/// Optional Postgres pool when `DATABASE_URL` is configured.
pub async fn maybe_init_database() -> anyhow::Result<Option<sqlx::PgPool>> {
    match std::env::var("DATABASE_URL") {
        Ok(url) => {
            let pool = crate::db_bootstrap::connect_and_migrate(&url).await?;
            Ok(Some(pool))
        }
        Err(_) => Ok(None),
    }
}

/// Build platform + shared approval workflow for gateway handlers.
pub async fn build_platform_with_approval() -> anyhow::Result<(Arc<Platform>, Arc<ApprovalWorkflow>)>
{
    let approval_workflow = Arc::new(ApprovalWorkflow::new());

    let execution: Arc<dyn ExecutionClient> = if let Ok(url) = std::env::var("WORKER_URL") {
        tracing::info!("gateway using remote worker at {url}");
        Arc::new(HttpExecutionClient::new(url))
    } else {
        tracing::info!("gateway using in-process worker (standalone mode)");
        let service = Arc::new(WorkerService::new_with_approval(approval_workflow.clone()).await?);
        clawz_worker::cron::spawn_cron_scheduler(service.clone());
        clawz_worker::background::spawn_subconscious_scheduler(service.clone());
        let platform = Arc::new(Platform::new(Arc::new(InProcessExecutionClient::new(
            service.clone(),
        ))));
        crate::turn_event_bridge::spawn_turn_event_bridge(service, platform.clone());
        return Ok((platform, approval_workflow));
    };

    Ok((Arc::new(Platform::new(execution)), approval_workflow))
}

/// Backward-compatible helper.
pub async fn build_platform() -> anyhow::Result<Arc<Platform>> {
    Ok(build_platform_with_approval().await?.0)
}

/// Agent container scheduler for fleet deploy (standalone in-process or Docker).
pub fn build_agent_scheduler() -> anyhow::Result<Arc<dyn AgentScheduler>> {
    let mode = DeploymentMode::from_env();
    create_scheduler(mode).map_err(|e| anyhow::anyhow!("{e}"))
}
