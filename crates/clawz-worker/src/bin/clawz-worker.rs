//! `clawz-worker` — standalone agent worker entry point for ClawZ.
//!
//! Run with: `cargo run --bin clawz-worker`
//!
//! This binary starts a self-contained agent worker that bootstrapping
//! AgentRuntime with the same dependency wiring as the Tauri desktop shell.

use clawz_core::types::AgentConfig;
use clawz_platform::detect_platform_fallback;
use clawz_worker::runtime::{agent::AgentRuntime, RuntimeDependencies};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

// Re-export the types we need from clawz_worker lib at the top of the binary
// so the local impl block can reference them without path ambiguity.
use clawz_worker::self_healing::{SupervisorConfig, CircuitBreakerScheduler, run_with_supervisor};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let tier = detect_platform_fallback();
    tracing::info!("clawz-worker starting on tier {:?}", tier);

    let runtime = build_runtime(tier).await?;

    let _supervised = run_with_supervisor(
        SupervisedAgentRuntime { inner: runtime },
        SupervisorConfig::default(),
    );

    tracing::info!("clawz-worker running — press Ctrl+C to stop");
    tokio::signal::ctrl_c().await?;
    tracing::info!("clawz-worker shutting down");
    Ok(())
}

async fn build_runtime(
    tier: clawz_core::PlatformTier,
) -> anyhow::Result<AgentRuntime> {
    let config = AgentConfig::new("clawz-worker", "claude-sonnet-4-5")
        .with_system_prompt(system_prompt_for_tier(tier));

    let router = build_provider_router().await?;

    let deps = RuntimeDependencies::new(
        Arc::new(router),
        Arc::new(clawz_worker::memory::store::InMemoryBackend::new()),
        Arc::new(clawz_worker::governance::engine::ClawzGovernanceEngine::new(
            clawz_worker::governance::engine::GovernanceEngineConfig::default(),
        )),
        Arc::new(clawz_worker::providers::CostTracker::new()),
    )
        .with_mbti_drift_detector(Arc::new(
            clawz_worker::runtime::mbti_drift_detector::MBTIDriftDetector::new(3, 0.75),
        ))
        .with_identity_store(Arc::new(
            clawz_worker::runtime::identity::AgentIdentityStore::new_in_memory(),
        ));

    Ok(AgentRuntime::new(config, deps))
}

async fn build_provider_router()
    -> anyhow::Result<clawz_worker::providers::router::ProviderRouter>
{
    use clawz_worker::providers::router::{ProviderRouterConfig, ReliabilityConfig};

    let mut providers = std::collections::HashMap::new();

    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        providers.insert(
            "openai".to_string(),
            clawz_worker::providers::router::ProviderConfig {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: key,
                models: vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()],
                ..Default::default()
            },
        );
    }

    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        providers.insert(
            "anthropic".to_string(),
            clawz_worker::providers::router::ProviderConfig {
                endpoint: "https://api.anthropic.com/v1".to_string(),
                api_key: key,
                models: vec![
                    "claude-sonnet-4-5".to_string(),
                    "claude-opus-4-7".to_string(),
                ],
                ..Default::default()
            },
        );
    }

    let reliability = ReliabilityConfig {
        max_retries: 3,
        base_delay_ms: 500,
        max_delay_ms: 30_000,
        exponential_base: 2.0,
        jitter: 0.1,
        circuit_breaker_threshold: 5,
        circuit_breaker_timeout_secs: 30,
    };

    let config = ProviderRouterConfig {
        providers,
        reliability,
        budget: None,
    };

    Ok(
        clawz_worker::providers::router::ProviderRouter::new(config)
            .await
            .map_err(|e| anyhow::anyhow!("provider router error: {e}"))?,
    )
}

fn system_prompt_for_tier(tier: clawz_core::PlatformTier) -> String {
    match tier {
        clawz_core::PlatformTier::T0 => {
            "You are ClawZ on ESP32. Be extremely concise.".to_string()
        }
        clawz_core::PlatformTier::T1 => {
            "You are ClawZ on a single-board computer. Be concise.".to_string()
        }
        clawz_core::PlatformTier::T2 => {
            "You are ClawZ in a container.".to_string()
        }
        clawz_core::PlatformTier::T3 => {
            "You are ClawZ, a helpful AI assistant in a desktop app.".to_string()
        }
    }
}

/// Wrapper that makes AgentRuntime implement CircuitBreakerScheduler.
struct SupervisedAgentRuntime {
    inner: AgentRuntime,
}

impl CircuitBreakerScheduler for SupervisedAgentRuntime {
    fn should_open_circuit(&self) -> bool {
        false
    }

    fn record_success(&mut self) {}

    fn record_failure(&mut self) {}

    async fn recover(&mut self) -> clawz_core::error::Result<()> {
        Ok(())
    }

    async fn tick(&mut self) -> clawz_core::error::Result<()> {
        Ok(())
    }
}
