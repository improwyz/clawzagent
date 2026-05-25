//! JSON-RPC 2.0 MCP server implementation.
//!
//! This file defines the wire-protocol types, built-in tool and resource
//! registries, and the request dispatcher that powers the gateway's MCP
//! endpoint. It is mounted as an Axum route handler by the parent gateway
//! HTTP router.
//!
//! # Protocol coverage
//!
//! Implements MCP over JSON-RPC 2.0, supporting:
//! - `initialize`
//! - `tools/list` and `tools/call`
//! - `resources/list` and `resources/read`
//! - `notifications/initialized`
//!
//! # Cross-module integration
//!
//! // Dependency: gateway HTTP router (crate root) mounts [`handle_mcp_request`].
//! // Dependency: gateway services would be called by `execute_tool` and `read_resource` in production.

use axum::Json;
// Dependency: `serde` and `serde_json` for JSON-RPC wire serialization.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 wire types
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 request envelope received from an MCP client.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version; must be exactly `"2.0"`.
    pub jsonrpc: String,
    /// Request identifier used to correlate responses. `None` for notifications.
    pub id: Option<Value>,
    /// The MCP method being invoked (e.g. `"tools/list"`, `"resources/read"`).
    pub method: String,
    /// Method-specific parameters. Omitted for methods that require no arguments.
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response envelope returned to an MCP client.
///
/// For every request, exactly one of `result` or `error` is populated.
/// Both may be absent for notifications, which have no response per spec.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    /// Protocol version; always `"2.0"`.
    pub jsonrpc: String,
    /// Identifier copied from the corresponding request. `None` for notifications.
    pub id: Option<Value>,
    /// Successful result payload. Omitted when `error` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error details. Omitted when `result` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object returned when an MCP request fails.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    /// Numeric error code defined by the JSON-RPC 2.0 spec (e.g. `-32601`).
    pub code: i32,
    /// Human-readable description of what went wrong.
    pub message: String,
    /// Optional structured error data. Omitted when not needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    /// Build a successful JSON-RPC response carrying a result payload.
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Build a failed JSON-RPC response carrying an error payload.
    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in tools exposed via MCP
// ---------------------------------------------------------------------------

/// Return the static registry of built-in tools exposed by this MCP server.
///
/// Each entry describes a callable operation (e.g. `agent_status`,
/// `dispatch_task`) with a JSON Schema input definition. These tools act
/// as the remote-control surface for the gateway's agent orchestration.
fn builtin_tools() -> Value {
    json!([
        {
            "name": "agent_status",
            "description": "Get the status of a ClawZ agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "The ID of the agent to query."
                    }
                },
                "required": ["agent_id"]
            }
        },
        {
            "name": "list_agents",
            "description": "List all registered ClawZ agents.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "dispatch_task",
            "description": "Dispatch a task to a ClawZ agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "Target agent ID."
                    },
                    "task": {
                        "type": "string",
                        "description": "Natural-language task description."
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["low", "normal", "high", "critical"],
                        "description": "Task priority."
                    }
                },
                "required": ["agent_id", "task"]
            }
        },
        {
            "name": "get_metrics",
            "description": "Get current gateway metrics snapshot.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "approve_action",
            "description": "Approve or reject a pending agent action.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "approval_id": {
                        "type": "string",
                        "description": "The approval request ID."
                    },
                    "decision": {
                        "type": "string",
                        "enum": ["approve", "reject"],
                        "description": "The decision."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Optional reason for the decision."
                    }
                },
                "required": ["approval_id", "decision"]
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Built-in resources exposed via MCP
// ---------------------------------------------------------------------------

/// Return the static registry of built-in resources exposed by this MCP server.
///
/// Resources are read-only data sources identified by `clawz://` URIs. They
/// provide snapshots of gateway state such as the agent registry, provider
/// list, and recent logs.
fn builtin_resources() -> Value {
    json!([
        {
            "uri": "clawz://agents",
            "name": "Agent Registry",
            "description": "List of all registered agents and their current state.",
            "mimeType": "application/json"
        },
        {
            "uri": "clawz://providers",
            "name": "Provider Registry",
            "description": "Available LLM providers and their status.",
            "mimeType": "application/json"
        },
        {
            "uri": "clawz://metrics",
            "name": "Gateway Metrics",
            "description": "Current gateway performance metrics.",
            "mimeType": "application/json"
        },
        {
            "uri": "clawz://governance/policies",
            "name": "Governance Policies",
            "description": "Active governance and compliance policies.",
            "mimeType": "application/json"
        },
        {
            "uri": "clawz://logs/recent",
            "name": "Recent Logs",
            "description": "Recent structured log entries.",
            "mimeType": "application/json"
        }
    ])
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

/// Execute a built-in tool by name with the provided arguments.
///
/// **Note:** All implementations below return hard-coded demo / stub data.
/// In a production deployment this dispatcher would call into the gateway's
/// internal services (e.g. agent manager, task scheduler, metrics collector).
fn execute_tool(name: &str, args: &Value) -> Value {
    match name {
        "agent_status" => {
            // Safely extract the agent_id; default to "unknown" so the stub
            // response never panics on malformed input.
            let agent_id = args["agent_id"].as_str().unwrap_or("unknown");
            json!({
                "agent_id": agent_id,
                "status": "running",
                "uptime_secs": 3742,
                "tasks_completed": 17,
                "current_task": null
            })
        }
        "list_agents" => json!({
            "agents": [
                {"id": "agent-001", "name": "ResearchAgent", "status": "running", "provider": "anthropic"},
                {"id": "agent-002", "name": "CodeAgent", "status": "idle", "provider": "openai"},
                {"id": "agent-003", "name": "DataAgent", "status": "starting", "provider": "anthropic"},
            ],
            "total": 3
        }),
        "dispatch_task" => {
            // Gracefully fall back to defaults so stub dispatch never panics
            // even when required fields are missing.
            let agent_id = args["agent_id"].as_str().unwrap_or("unknown");
            let task = args["task"].as_str().unwrap_or("");
            let priority = args["priority"].as_str().unwrap_or("normal");
            json!({
                "task_id": format!("task-{}", uuid::Uuid::new_v4()),
                "agent_id": agent_id,
                "task": task,
                "priority": priority,
                "status": "queued",
                "queued_at": chrono::Utc::now().to_rfc3339()
            })
        }
        "get_metrics" => json!({
            "active_agents": 3,
            "requests_per_min": 42,
            "avg_latency_ms": 120,
            "memory_mb": 512,
            "cpu_percent": 18,
            "uptime_secs": 9432,
            "snapshot_at": chrono::Utc::now().to_rfc3339()
        }),
        "approve_action" => {
            // Default to "reject" on malformed input as a safe conservative
            // choice for approval stubs.
            let approval_id = args["approval_id"].as_str().unwrap_or("unknown");
            let decision = args["decision"].as_str().unwrap_or("reject");
            json!({
                "approval_id": approval_id,
                "decision": decision,
                "processed_at": chrono::Utc::now().to_rfc3339(),
                "status": "processed"
            })
        }
        _ => json!({"error": format!("Unknown tool: {}", name)}),
    }
}

// ---------------------------------------------------------------------------
// Resource reading
// ---------------------------------------------------------------------------

/// Read a built-in resource by its `clawz://` URI.
///
/// Returns `Some(Value)` containing the resource contents, or `None` if the
/// URI is not recognized by the gateway.
fn read_resource(uri: &str) -> Option<Value> {
    match uri {
        "clawz://agents" => Some(json!({
            "agents": [
                {"id": "agent-001", "name": "ResearchAgent", "status": "running"},
                {"id": "agent-002", "name": "CodeAgent", "status": "idle"},
            ]
        })),
        "clawz://providers" => Some(json!({
            "providers": [
                {"name": "anthropic", "status": "connected", "models": ["claude-sonnet-4-6"]},
                {"name": "openai", "status": "connected", "models": ["gpt-4o"]},
            ]
        })),
        "clawz://metrics" => Some(json!({
            "active_agents": 2,
            "requests_per_min": 38,
            "avg_latency_ms": 115,
            "snapshot_at": chrono::Utc::now().to_rfc3339()
        })),
        "clawz://governance/policies" => Some(json!({
            "policies": [
                {"id": "pol-001", "name": "RateLimitPolicy", "enabled": true},
                {"id": "pol-002", "name": "ApprovalPolicy", "enabled": true},
            ]
        })),
        "clawz://logs/recent" => Some(json!({
            "lines": [
                {"level": "INFO", "message": "Agent started", "agent_id": "agent-001"},
                {"level": "DEBUG", "message": "Request received", "path": "/api/agents"},
            ],
            "count": 2
        })),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Main dispatcher
// ---------------------------------------------------------------------------

/// Axum handler that deserializes an incoming JSON-RPC 2.0 MCP request,
/// dispatches it to the appropriate method implementation, and returns
/// the serialized response.
///
/// This is the primary integration point between the gateway HTTP layer and
/// the MCP protocol engine.
pub async fn handle_mcp_request(Json(req): Json<JsonRpcRequest>) -> Json<JsonRpcResponse> {
    // Reject requests that do not conform to the JSON-RPC 2.0 spec upfront.
    if req.jsonrpc != "2.0" {
        return Json(JsonRpcResponse::err(
            req.id,
            -32600,
            "Invalid Request: jsonrpc must be \"2.0\"",
        ));
    }

    // Default to an empty object so downstream handlers can unconditionally
    // index into `params` without checking `None` every time.
    let params = req.params.clone().unwrap_or(json!({}));

    let response = match req.method.as_str() {
        // ------------------------------------------------------------------
        // Handshake: tells the client which MCP protocol version and
        // capabilities (tools, resources) this server supports.
        "initialize" => JsonRpcResponse::ok(
            req.id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {}
                },
                "serverInfo": {
                    "name": "clawz-gateway",
                    "version": "0.1.0"
                }
            }),
        ),

        // ------------------------------------------------------------------
        // Return the full catalog of callable tools.
        "tools/list" => JsonRpcResponse::ok(
            req.id,
            json!({ "tools": builtin_tools() }),
        ),

        // ------------------------------------------------------------------
        // Invoke a single tool by name with the arguments supplied by the client.
        "tools/call" => {
            let tool_name = match params["name"].as_str() {
                Some(n) => n.to_string(),
                None => {
                    return Json(JsonRpcResponse::err(
                        req.id,
                        -32602,
                        "Invalid params: missing \"name\"",
                    ))
                }
            };
            // Clone the arguments object so `execute_tool` owns its input;
            // this keeps the borrow checker happy while we continue using `params`.
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            let result = execute_tool(&tool_name, &args);
            JsonRpcResponse::ok(
                req.id,
                json!({
                    "content": [{
                        "type": "text",
                        // Pretty-print for human readability in MCP clients;
                        // fall back to compact JSON if pretty-printing fails.
                        "text": serde_json::to_string_pretty(&result)
                            .unwrap_or_else(|_| result.to_string())
                    }]
                }),
            )
        }

        // ------------------------------------------------------------------
        // Return the full catalog of readable resources.
        "resources/list" => JsonRpcResponse::ok(
            req.id,
            json!({ "resources": builtin_resources() }),
        ),

        // ------------------------------------------------------------------
        // Read a single resource by URI and return its contents.
        "resources/read" => {
            let uri = match params["uri"].as_str() {
                Some(u) => u.to_string(),
                None => {
                    return Json(JsonRpcResponse::err(
                        req.id,
                        -32602,
                        "Invalid params: missing \"uri\"",
                    ))
                }
            };
            match read_resource(&uri) {
                Some(content) => JsonRpcResponse::ok(
                    req.id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            // Pretty-print for human readability; fallback to
                            // compact representation if serialization fails.
                            "text": serde_json::to_string_pretty(&content)
                                .unwrap_or_else(|_| content.to_string())
                        }]
                    }),
                ),
                None => JsonRpcResponse::err(
                    req.id,
                    -32001,
                    format!("Resource not found: {}", uri),
                ),
            }
        }

        // ------------------------------------------------------------------
        // notifications/initialized — no response per spec, but we ack anyway
        "notifications/initialized" => {
            // Per MCP spec, notifications have no id and require no response.
            // Return a no-op response (caller should ignore it for notifications).
            return Json(JsonRpcResponse::ok(req.id, json!(null)));
        }

        // ------------------------------------------------------------------
        // Any method not listed above is unsupported.
        _ => JsonRpcResponse::err(
            req.id,
            -32601,
            format!("Method not found: {}", req.method),
        ),
    };

    Json(response)
}

#[cfg(test)]
mod tests {
    //! Unit tests for the MCP JSON-RPC dispatcher.
    //!
    //! Covers happy-path initialization, tool/resource listing and invocation,
    //! plus error cases for unknown methods and invalid protocol versions.

    use super::*;
    use serde_json::json;

    /// Build a test request with the given method and optional params.
    fn make_req(method: &str, params: Option<Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: method.into(),
            params,
        }
    }

    /// Verify that `initialize` returns the expected protocol version and
    /// server info payload.
    #[tokio::test]
    async fn test_initialize() {
        let req = make_req("initialize", None);
        let Json(resp) = handle_mcp_request(Json(req)).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "clawz-gateway");
    }

    /// Verify that `tools/list` returns a non-empty array of tool definitions.
    #[tokio::test]
    async fn test_tools_list() {
        let req = make_req("tools/list", None);
        let Json(resp) = handle_mcp_request(Json(req)).await;
        assert!(resp.error.is_none());
        let tools = &resp.result.unwrap()["tools"];
        assert!(tools.as_array().unwrap().len() > 0);
    }

    /// Verify that `tools/call` invokes a tool and wraps the result in text
    /// content as required by the MCP protocol.
    #[tokio::test]
    async fn test_tools_call() {
        let req = make_req(
            "tools/call",
            Some(json!({"name": "list_agents", "arguments": {}})),
        );
        let Json(resp) = handle_mcp_request(Json(req)).await;
        assert!(resp.error.is_none());
        let content = &resp.result.unwrap()["content"];
        assert_eq!(content[0]["type"], "text");
    }

    /// Verify that `resources/list` returns a non-empty array of resource
    /// definitions.
    #[tokio::test]
    async fn test_resources_list() {
        let req = make_req("resources/list", None);
        let Json(resp) = handle_mcp_request(Json(req)).await;
        assert!(resp.error.is_none());
        let resources = &resp.result.unwrap()["resources"];
        assert!(resources.as_array().unwrap().len() > 0);
    }

    /// Verify that `resources/read` successfully fetches a known resource.
    #[tokio::test]
    async fn test_resources_read() {
        let req = make_req(
            "resources/read",
            Some(json!({"uri": "clawz://agents"})),
        );
        let Json(resp) = handle_mcp_request(Json(req)).await;
        assert!(resp.error.is_none());
    }

    /// Verify that an unknown method yields a JSON-RPC `-32601` error.
    #[tokio::test]
    async fn test_unknown_method() {
        let req = make_req("unknown/method", None);
        let Json(resp) = handle_mcp_request(Json(req)).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    /// Verify that a non-2.0 `jsonrpc` version yields a JSON-RPC `-32600`
    /// invalid-request error.
    #[tokio::test]
    async fn test_invalid_jsonrpc_version() {
        let req = JsonRpcRequest {
            jsonrpc: "1.0".into(),
            id: Some(json!(1)),
            method: "initialize".into(),
            params: None,
        };
        let Json(resp) = handle_mcp_request(Json(req)).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }
}
