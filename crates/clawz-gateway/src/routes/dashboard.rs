//! JSON dashboard metrics and config consumed by the web UI.

use axum::{Json, Router, extract::State, routing::get};
use chrono::Utc;
use serde_json::{Value, json};

use crate::{AgentStatus, AppState, ChannelRecord, ProviderRecord};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/metrics", get(dashboard_metrics))
        .route("/config", get(dashboard_config))
        .route("/tools", get(dashboard_tools))
        .merge(crate::routes::dashboard_overview::routes())
}

/// Mask a stored API key for display (never return the full secret).
fn mask_api_key(key: Option<&String>) -> Option<String> {
    let k = key.as_ref()?;
    if k.is_empty() {
        return None;
    }
    if k.len() <= 8 {
        return Some("********".to_string());
    }
    Some(format!("{}…{}", &k[..4], &k[k.len() - 4..]))
}

fn provider_json(p: &ProviderRecord) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "type": p.provider_type,
        "provider_type": p.provider_type,
        "api_key_masked": mask_api_key(p.api_key.as_ref()),
        "base_url": p.base_url,
        "enabled": p.enabled,
        "created_at": p.created_at,
        "updated_at": p.updated_at,
    })
}

fn channel_json(c: &ChannelRecord) -> Value {
    let status = if !c.enabled { "idle" } else { "active" };
    json!({
        "id": c.id,
        "name": c.name,
        "type": c.channel_type,
        "channel_type": c.channel_type,
        "status": status,
        "enabled": c.enabled,
        "config": c.config,
        "tenant_id": c.tenant_id,
    })
}

fn cloudflare_services() -> Value {
    let has_token = std::env::var("CLOUDFLARE_API_TOKEN")
        .or_else(|_| std::env::var("CF_API_TOKEN"))
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let master = std::env::var("CLAWZ_CLOUDFLARE_ENABLED")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(has_token);

    fn svc_enabled(master: bool, key: &str) -> bool {
        if !master {
            return false;
        }
        std::env::var(key)
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(master)
    }

    json!({
        "ai_gateway": svc_enabled(master, "CLAWZ_CF_AI_GATEWAY_ENABLED"),
        "r2": svc_enabled(master, "CLAWZ_CF_R2_ENABLED"),
        "kv": svc_enabled(master, "CLAWZ_CF_KV_ENABLED"),
        "tunnel": svc_enabled(master, "CLAWZ_CF_TUNNEL_ENABLED"),
        "workers": svc_enabled(master, "CLAWZ_CF_WORKERS_ENABLED"),
        "containers": svc_enabled(master, "CLAWZ_CF_CONTAINERS_ENABLED"),
        "configured": has_token,
    })
}

const SAAS_CATALOG: &[(&str, &str, &str)] = &[
    ("slack", "Slack", "💬"),
    ("github", "GitHub", "🐙"),
    ("gitlab", "GitLab", "🦊"),
    ("google_workspace", "Google Workspace", "📧"),
    ("microsoft365", "Microsoft 365", "Ⓜ️"),
    ("salesforce", "Salesforce", "☁"),
    ("hubspot", "HubSpot", "🟠"),
    ("stripe", "Stripe", "💳"),
    ("notion", "Notion", "📝"),
    ("linear", "Linear", "◆"),
    ("atlassian", "Atlassian (Jira)", "📋"),
    ("twilio", "Twilio", "📞"),
    ("shopify", "Shopify", "🛒"),
    ("datadog", "Datadog", "🐕"),
    ("intercom", "Intercom", "💡"),
];

fn saas_connectors() -> Vec<Value> {
    SAAS_CATALOG
        .iter()
        .map(|(id, name, icon)| {
            let env_key = format!(
                "CLAWZ_CONNECTOR_{}_CONNECTED",
                id.to_uppercase().replace('.', "_")
            );
            let connected = std::env::var(&env_key)
                .ok()
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            json!({
                "id": id,
                "name": name,
                "icon": icon,
                "connected": connected,
                "env_hint": env_key,
            })
        })
        .collect()
}

fn gateway_port() -> u16 {
    std::env::var("CLAWZ__SERVER__PORT")
        .or_else(|_| std::env::var("PORT"))
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000)
}

/// Shared snapshot for `GET /dashboard/metrics` and `/ws/metrics`.
pub async fn snapshot_dashboard_metrics(state: &AppState) -> Value {
    let agents = state.agents.read().await;
    let conversations = state.conversations.read().await;
    let nodes = state.fleet_nodes.read().await;

    let active = agents
        .iter()
        .filter(|a| matches!(a.status, AgentStatus::Running))
        .count();

    json!({
        "active_agents": active,
        "total_conversations": conversations.len(),
        "requests_per_min": 0,
        "avg_latency_ms": 0,
        "requests_over_time": [],
        "provider_distribution": [],
        "fleet_nodes": nodes.len(),
        "generated_at": Utc::now().to_rfc3339(),
    })
}

/// `GET /dashboard/metrics` — JSON metrics for the React dashboard.
async fn dashboard_metrics(State(state): State<AppState>) -> Json<Value> {
    Json(snapshot_dashboard_metrics(&state).await)
}

/// `GET /dashboard/config` — aggregated settings for the Config UI.
async fn dashboard_config(State(state): State<AppState>) -> Json<Value> {
    let providers = state.providers.read().await;
    let channels = state.channels.read().await;

    let default_provider = providers
        .iter()
        .find(|p| p.enabled)
        .map(|p| p.name.clone())
        .or_else(|| std::env::var("CLAWZ_DEFAULT_PROVIDER").ok())
        .unwrap_or_else(|| "none".to_string());

    let docker_registry =
        std::env::var("CLAWZ_DOCKER_REGISTRY").unwrap_or_else(|_| "ghcr.io/improwyz".to_string());

    let ui = state.ui_settings.read().await;

    Json(json!({
        "providers": providers.iter().map(provider_json).collect::<Vec<_>>(),
        "channels": channels.iter().map(channel_json).collect::<Vec<_>>(),
        "default_provider": default_provider,
        "docker_registry": docker_registry,
        "cloudflare": cloudflare_services(),
        "saas_connectors": saas_connectors(),
        "system": {
            "port": gateway_port(),
            "jwt_secret_masked": "********",
            "log_level": ui.log_level,
            "max_agents": ui.max_agents,
            "enable_audit": ui.enable_audit,
            "auth_disabled": std::env::var("CLAWZ_DISABLE_AUTH").ok().as_deref() == Some("1"),
            "gateway_version": env!("CARGO_PKG_VERSION"),
        },
    }))
}

/// `GET /dashboard/tools` — tool catalog, Docker library, and MCP servers for the UI.
async fn dashboard_tools(State(state): State<AppState>) -> Json<Value> {
    Json(crate::tool_catalog::snapshot_dashboard_tools(&state).await)
}
