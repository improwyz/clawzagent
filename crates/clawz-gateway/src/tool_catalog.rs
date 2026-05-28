//! Built-in tool, Docker, and MCP catalogs for the dashboard UI.

use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ToolRecord;

/// Built-in agent tools shipped with ClawZ.
pub const BUILTIN_TOOLS: &[(&str, &str, &str)] = &[
    ("web_fetch", "web", "HTTP GET/POST requests with response parsing"),
    ("web_search", "search", "Search the web via configured search API"),
    ("browser", "web", "Browser automation via Chrome DevTools Protocol"),
    ("file_ops", "file", "Read, write, list, and delete workspace files"),
    ("shell", "code", "Execute shell commands in a sandboxed environment"),
    ("git", "code", "Git clone, commit, push, and pull operations"),
    ("calculator", "code", "Evaluate mathematical expressions"),
    ("image_gen", "api", "Generate images via configured image API"),
    ("pdf_read", "file", "Extract text and metadata from PDF documents"),
    ("knowledge_base", "database", "Semantic search over uploaded documents"),
    ("escalate", "api", "Request human approval or intervention"),
];

const MCP_CATALOG: &[(&str, &str, &str)] = &[
    (
        "clawz-gateway",
        "ClawZ Gateway MCP",
        "POST /api/v1/mcp on this gateway",
    ),
    (
        "filesystem",
        "Filesystem MCP",
        "stdio — local file read/write (configure command in tool config)",
    ),
    (
        "github",
        "GitHub MCP",
        "https://api.githubcopilot.com/mcp/ (requires token)",
    ),
    (
        "postgres",
        "PostgreSQL MCP",
        "stdio — SQL queries against your database",
    ),
];

/// Seed in-memory tool registry when empty (fresh install).
pub fn default_tool_records() -> Vec<ToolRecord> {
    let now = Utc::now();
    BUILTIN_TOOLS
        .iter()
        .map(|(name, tool_type, description)| ToolRecord {
            id: Uuid::new_v4().to_string(),
            name: (*name).to_string(),
            description: (*description).to_string(),
            tool_type: (*tool_type).to_string(),
            config: json!({ "source": "builtin" }),
            enabled: true,
            created_at: now,
            updated_at: now,
        })
        .collect()
}

pub async fn ensure_default_tools(state: &crate::AppState) {
    let mut tools = state.tools.write().await;
    if !tools.is_empty() {
        return;
    }
    *tools = default_tool_records();
    drop(tools);
    if let Some(ref pool) = state.db {
        let tools = state.tools.read().await;
        for record in tools.iter() {
            let _ = crate::postgres_store::persist_tool(pool, record).await;
        }
    }
    tracing::info!(
        "seeded {} built-in tools",
        state.tools.read().await.len()
    );
}

fn tool_to_catalog_json(t: &ToolRecord) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "category": t.tool_type,
        "tool_type": t.tool_type,
        "description": t.description,
        "enabled": t.enabled,
        "installed": true,
        "source": "registered",
    })
}

/// Build dashboard tools payload (catalog, docker library, MCP servers).
pub async fn snapshot_dashboard_tools(state: &crate::AppState) -> Value {
    ensure_default_tools(state).await;

    let tools = state.tools.read().await;
    let registered_names: std::collections::HashSet<String> =
        tools.iter().map(|t| t.name.clone()).collect();

    let mut catalog: Vec<Value> = tools.iter().map(tool_to_catalog_json).collect();

    for (name, tool_type, description) in BUILTIN_TOOLS {
        if registered_names.contains(*name) {
            continue;
        }
        catalog.push(json!({
            "id": format!("catalog-{name}"),
            "name": name,
            "category": tool_type,
            "tool_type": tool_type,
            "description": description,
            "enabled": false,
            "installed": false,
            "source": "catalog",
        }));
    }

    let docker_tools: Vec<Value> = clawz_worker::tools::docker::docker_tool_library()
        .into_iter()
        .map(|e| {
            let port = e.ports.first().copied();
            json!({
                "id": format!("docker-{}", e.name),
                "name": e.name,
                "image": e.image,
                "status": "stopped",
                "port": port,
                "category": e.category,
                "description": e.description,
            })
        })
        .collect();

    let mut mcp_servers: Vec<Value> = tools
        .iter()
        .filter(|t| t.tool_type == "mcp")
        .map(|t| {
            let url = t
                .config
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            json!({
                "id": t.id,
                "name": t.name,
                "url": url,
                "status": if t.enabled { "connected" } else { "disconnected" },
                "tools_count": t.config.get("tools_count").and_then(|v| v.as_u64()).unwrap_or(0),
                "registered": true,
            })
        })
        .collect();

    for (id, name, url) in MCP_CATALOG {
        if mcp_servers
            .iter()
            .any(|s| s.get("name").and_then(|v| v.as_str()) == Some(name))
        {
            continue;
        }
        mcp_servers.push(json!({
            "id": format!("mcp-{id}"),
            "name": name,
            "url": url,
            "status": "disconnected",
            "tools_count": 0,
            "registered": false,
        }));
    }

    let tools_json: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "category": t.tool_type,
                "tool_type": t.tool_type,
                "description": t.description,
                "enabled": t.enabled,
            })
        })
        .collect();

    json!({
        "tools": tools_json,
        "catalog": catalog,
        "docker_tools": docker_tools,
        "mcp_servers": mcp_servers,
    })
}
