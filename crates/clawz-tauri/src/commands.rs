//! Tauri IPC command handlers — bridge between frontend and ClawZ agent.

use clawz_core::types::{AgentConfig, Message};
use clawz_platform::detect_platform_fallback;
use clawz_worker::runtime::{agent::AgentRuntime, RuntimeDependencies};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Global app state accessed from Tauri command handlers.
/// Set once at startup in `main.rs` before the app loop begins.
static APP_STATE: std::sync::OnceLock<AppState> = std::sync::OnceLock::new();

/// Set the global app state. Must be called before any commands are invoked.
pub fn set_app_state(state: AppState) {
    APP_STATE.set(state).ok();
}

/// Fetch a clone of the current runtime, if initialised.
fn get_runtime() -> Option<Arc<AgentRuntime>> {
    APP_STATE
        .get()
        .and_then(|s| s.runtime.lock().unwrap().clone())
}

/// Application state shared across all Tauri commands.
pub struct AppState {
    /// The agent runtime, populated asynchronously after startup.
    runtime: Mutex<Option<Arc<AgentRuntime>>>,
    /// Detected platform tier for this binary.
    pub platform_tier: clawz_core::PlatformTier,
    /// Version string from Cargo.toml.
    pub version: String,
}

impl AppState {
    /// Build a new app state with the runtime pre-warmed.
    pub async fn new() -> Self {
        let platform_tier = detect_platform_fallback();
        let version = env!("CARGO_PKG_VERSION").to_string();
        let runtime = Self::build_runtime(platform_tier).await;
        Self {
            runtime: Mutex::new(runtime),
            platform_tier,
            version,
        }
    }

    /// Construct an [`AgentRuntime`] for the given platform tier.
    async fn build_runtime(tier: clawz_core::PlatformTier) -> Option<Arc<AgentRuntime>> {
        let config = AgentConfig::new("clawz-default", "claude-sonnet-4-5")
            .with_system_prompt(Self::system_prompt_for_tier(tier));

        let router = Self::build_provider_router(tier).await;

        let deps = RuntimeDependencies::new(
            Arc::new(router),
            Arc::new(clawz_worker::memory::store::InMemoryBackend::new()),
            Arc::new(
                clawz_worker::governance::engine::ClawzGovernanceEngine::new(
                    clawz_worker::governance::engine::GovernanceEngineConfig::default(),
                ),
            ),
            Arc::new(clawz_worker::providers::CostTracker::new()),
        )
        .with_mbti_drift_detector(Arc::new(
            clawz_worker::runtime::mbti_drift_detector::MBTIDriftDetector::new(3, 0.75),
        ))
        .with_identity_store(Arc::new(
            clawz_worker::runtime::identity::AgentIdentityStore::new_in_memory(),
        ));

        Some(Arc::new(AgentRuntime::new(config, deps)))
    }

    async fn build_provider_router(
        _tier: clawz_core::PlatformTier,
    ) -> clawz_worker::providers::router::ProviderRouter {
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

        match clawz_worker::providers::router::ProviderRouter::new(config).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("failed to build provider router: {}", e);
                clawz_worker::providers::router::ProviderRouter::new(
                    clawz_worker::providers::router::ProviderRouterConfig::default(),
                )
                .await
                .expect("default router config should always succeed")
            }
        }
    }

    fn system_prompt_for_tier(tier: clawz_core::PlatformTier) -> String {
        match tier {
            clawz_core::PlatformTier::T0 => {
                "You are ClawZ on ESP32. Be extremely concise.".to_string()
            }
            clawz_core::PlatformTier::T1 => {
                "You are ClawZ on a single-board computer. Be concise.".to_string()
            }
            clawz_core::PlatformTier::T2 => "You are ClawZ in a container.".to_string(),
            clawz_core::PlatformTier::T3 => {
                "You are ClawZ, a helpful AI assistant in a desktop app.".to_string()
            }
        }
    }
}

// ── Serialisation types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub message: String,
    pub agent_id: String,
    pub tier: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub tier: String,
    pub tier_name: String,
    pub binary_budget_bytes: usize,
    pub ram_budget_bytes: usize,
    pub supports_containers: bool,
}

// ── Command handlers ─────────────────────────────────────────────────────────

// ── Command handlers ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn agent_chat(req: ChatRequest) -> Result<ChatResponse, String> {
    let agent_id = req.agent_id.unwrap_or_else(|| "default".to_string());

    let runtime = get_runtime()
        .ok_or_else(|| "agent runtime not yet initialised — check health endpoint".to_string())?;

    let message = Message::user(req.message);

    let messages = runtime
        .run_multi_turn(vec![message])
        .await
        .map_err(|e| e.to_string())?;

    let reply = messages
        .iter()
        .rev()
        .find(|m| m.role == clawz_core::types::Role::Assistant)
        .and_then(|m| m.content.as_text().map(String::from))
        .unwrap_or_else(|| "ClawZ is processing your request.".to_string());

    let tier = APP_STATE
        .get()
        .map(|s| format!("{:?}", s.platform_tier))
        .unwrap_or_else(|| "unknown".to_string());
    let version = APP_STATE
        .get()
        .map(|s| s.version.clone())
        .unwrap_or_else(|| "0.0.0".to_string());

    Ok(ChatResponse {
        message: reply,
        agent_id,
        tier,
        version,
    })
}

#[tauri::command]
pub fn get_platform_tier() -> PlatformInfo {
    APP_STATE
        .get()
        .map(|s| {
            let tier = s.platform_tier;
            PlatformInfo {
                tier: format!("{:?}", tier),
                tier_name: tier.name().to_string(),
                binary_budget_bytes: tier.binary_budget(),
                ram_budget_bytes: tier.ram_budget(),
                supports_containers: tier.supports_containers(),
            }
        })
        .unwrap_or_else(|| PlatformInfo {
            tier: "unknown".to_string(),
            tier_name: "unknown".to_string(),
            binary_budget_bytes: 0,
            ram_budget_bytes: 0,
            supports_containers: false,
        })
}

#[tauri::command]
pub fn health_check() -> Result<String, String> {
    let rt_status = APP_STATE
        .get()
        .and_then(|s| {
            let guard = s.runtime.lock().unwrap();
            guard.clone()
        })
        .map(|_| "runtime ready")
        .unwrap_or("runtime not yet initialised");

    let tier = APP_STATE
        .get()
        .map(|s| format!("{:?}", s.platform_tier))
        .unwrap_or_else(|| "unknown".to_string());
    let version = APP_STATE
        .get()
        .map(|s| s.version.clone())
        .unwrap_or_else(|| "0.0.0".to_string());

    Ok(format!("ClawZ {} — tier {} — {}", version, tier, rt_status))
}
