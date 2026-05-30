//! `clawz-gateway` — API gateway entry point for ClawZ agent platform.
//!
//! Run with: `cargo run --bin clawz-gateway`
//! Listens on `CLAWZ_LISTEN_ADDR` (default `0.0.0.0:3000`).

use std::net::SocketAddr;
use std::sync::Arc;

use clawz_core::deployment::DeploymentMode;
use clawz_gateway::bootstrap::{
    build_agent_scheduler, build_platform_with_approval, maybe_init_database,
};
use clawz_gateway::postgres_platform_store::PostgresPlatformStore;
use clawz_gateway::postgres_store;
use clawz_gateway::{AppState, server::GatewayServer};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let listen_addr: SocketAddr = std::env::var("CLAWZ_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3000".into())
        .parse()
        .expect("CLAWZ_LISTEN_ADDR must be a valid socket address");

    let jwt_secret = std::env::var("CLAWZ_JWT_SECRET")
        .or_else(|_| std::env::var("JWT_SECRET"))
        .unwrap_or_else(|_| "dev-secret-change-in-production".to_string());

    if jwt_secret == "dev-secret-change-in-production"
        && std::env::var("CLAWZ_MODE").unwrap_or_default() == "production"
    {
        anyhow::bail!("CLAWZ_JWT_SECRET must be set in production mode");
    }

    let identity_store = std::env::var("CLAWZ_IDENTITY_STORE").ok().map(|_path| {
        Arc::new(clawz_worker::runtime::identity::AgentIdentityStore::new_in_memory())
    });

    let db = maybe_init_database().await?;
    let (platform, approval_workflow) = build_platform_with_approval().await?;
    let platform_store = db.as_ref().map(|pool| {
        PostgresPlatformStore::new(pool.clone()) as Arc<dyn clawz_services::PlatformStore>
    });
    let agent_scheduler = if std::env::var("WORKER_URL").is_ok() {
        tracing::info!("fleet spawn delegated to worker (WORKER_URL set)");
        None
    } else {
        match build_agent_scheduler() {
            Ok(s) => {
                tracing::info!("fleet scheduler ready ({:?})", DeploymentMode::from_env());
                Some(s)
            }
            Err(e) => {
                tracing::warn!("fleet scheduler unavailable: {e}");
                None
            }
        }
    };
    let state = AppState::full(
        jwt_secret,
        identity_store,
        Some(platform),
        db.clone(),
        approval_workflow,
        platform_store,
        agent_scheduler,
    );
    if let Some(ref pool) = db {
        if let Ok(agents) = postgres_store::load_agents(pool).await {
            *state.agents.write().await = agents;
            tracing::info!(
                "hydrated {} agents from database",
                state.agents.read().await.len()
            );
        }
        if let Ok(conversations) = postgres_store::load_conversations(pool).await {
            *state.conversations.write().await = conversations;
            tracing::info!(
                "hydrated {} conversations from database",
                state.conversations.read().await.len()
            );
        }
        if let Ok(rooms) = postgres_store::load_all_rooms(pool).await {
            *state.rooms.write().await = rooms;
            tracing::info!(
                "hydrated {} rooms from database",
                state.rooms.read().await.len()
            );
        }
        if let Ok(users) = postgres_store::load_users(pool).await {
            *state.users.write().await = users;
            tracing::info!(
                "hydrated {} users from database",
                state.users.read().await.len()
            );
        }
        if let Ok(api_keys) = postgres_store::load_api_keys(pool).await {
            *state.api_keys.write().await = api_keys;
            tracing::info!(
                "hydrated {} api keys from database",
                state.api_keys.read().await.len()
            );
        }
        if let Ok(channels) = postgres_store::load_channels(pool).await {
            *state.channels.write().await = channels;
            tracing::info!(
                "hydrated {} channels from database",
                state.channels.read().await.len()
            );
        }
        if let Ok(nodes) = postgres_store::load_fleet_nodes(pool).await {
            *state.fleet_nodes.write().await = nodes;
            tracing::info!(
                "hydrated {} fleet nodes from database",
                state.fleet_nodes.read().await.len()
            );
        }
        if let Ok(policies) = postgres_store::load_policies(pool).await {
            *state.policies.write().await = policies;
            tracing::info!(
                "hydrated {} policies from database",
                state.policies.read().await.len()
            );
        }
        if let Ok(providers) = postgres_store::load_providers(pool).await {
            *state.providers.write().await = providers;
            tracing::info!(
                "hydrated {} providers from database",
                state.providers.read().await.len()
            );
        }
        if let Ok(tools) = postgres_store::load_tools(pool).await {
            *state.tools.write().await = tools;
            tracing::info!(
                "hydrated {} tools from database",
                state.tools.read().await.len()
            );
        }
        clawz_gateway::tool_catalog::ensure_default_tools(&state).await;
        if let Ok(mut audit) = postgres_store::load_audit_entries(pool).await {
            audit.reverse();
            *state.audit_log.write().await = audit;
            tracing::info!(
                "hydrated {} audit entries from database",
                state.audit_log.read().await.len()
            );
        }
        if let Ok(cloud) = postgres_store::load_cloud_deployments(pool).await {
            let mut restored = 0usize;
            for info in cloud {
                if state.deploy_manager.restore_deployment(info).is_ok() {
                    restored += 1;
                }
            }
            tracing::info!("hydrated {restored} cloud deployments from database");
        }
    } else {
        clawz_gateway::tool_catalog::ensure_default_tools(&state).await;
    }
    let state = std::sync::Arc::new(state);
    clawz_gateway::channel_supervisor::spawn(state.clone());
    clawz_gateway::connector_scheduler::spawn(state.clone());

    let server = GatewayServer::new((*state).clone());

    tracing::info!("clawz-gateway starting on {}", listen_addr);
    server.serve(listen_addr).await
}
