//! Model Context Protocol (MCP) integration for third-party tool servers.
//!
//! This module implements [`McpServerManager`] and [`McpClient`], which enable
//! agents to communicate with external MCP servers over stdio or HTTP transports,
//! discovering and invoking tools published by those servers.
//!
//! ## Features
//!
//! - **Server discovery**: Launch and connect to local MCP servers
//! - **Tool exposure**: Proxy tools from external servers into the agent runtime
//! - **Schema import**: Dynamically register server tools with their metadata
//! - **Request forwarding**: Forward agent tool calls to the appropriate server
//! - **Error handling**: Graceful fallback when servers fail or disconnect
//!
//! ## Supported Transports
//!
//! - **stdio**: Local processes communicating over stdin/stdout
//! - **SSE (HTTP)**: Server-Sent Events for remote server communication
//!
//! ## Cross-module dependencies
//!
//! // Dependency: `clawz_core::types::ToolSchema` for tool metadata.
//! // Dependency: Implements the [`Tool`] trait from [`super::tool_trait`].

use clawz_core::error::ClawzError;
use clawz_core::types::ToolSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── MCP Protocol Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: Value,
}

impl JsonRpcRequest {
    fn new(id: u64, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

/// MCP Resource descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// MCP Prompt descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArg {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
}

/// Information about an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
    pub capabilities: McpCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
    pub sampling: bool,
}

// ── Transport Layer ───────────────────────────────────────────────────────────

/// Transport modes for MCP server communication.
#[derive(Debug, Clone)]
pub enum McpTransport {
    /// JSON-RPC over stdio — spawn a subprocess
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    /// JSON-RPC over HTTP (SSE for streaming)
    Http {
        base_url: String,
        headers: HashMap<String, String>,
    },
}

// ── MCP Client (Stdio) ────────────────────────────────────────────────────────

/// Low-level MCP client that communicates via stdio with a subprocess.
struct StdioMcpClient {
    process: std::process::Child,
    stdin: std::process::ChildStdin,
    reader: std::io::BufReader<std::process::ChildStdout>,
    next_id: u64,
    initialized: bool,
}

impl StdioMcpClient {
    fn spawn(command: &str, args: &[String], env: &HashMap<String, String>) -> Result<Self, ClawzError> {
        let mut cmd = std::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ClawzError::Tool(format!("failed to spawn MCP server '{}': {e}", command)))?;

        let stdin = child.stdin.take().expect("no stdin");
        let stdout = child.stdout.take().expect("no stdout");

        Ok(Self {
            process: child,
            stdin,
            reader: std::io::BufReader::new(stdout),
            next_id: 1,
            initialized: false,
        })
    }

    fn send_request(&mut self, method: &str, params: Value) -> Result<Value, ClawzError> {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest::new(id, method, params);
        let json = serde_json::to_string(&req)?;

        // Write JSON-RPC request + newline (NDJSON framing)
        writeln!(self.stdin, "{}", json)
            .map_err(|e| ClawzError::Tool(format!("MCP write error: {e}")))?;
        self.stdin.flush()
            .map_err(|e| ClawzError::Tool(format!("MCP flush error: {e}")))?;

        // Read response(s) until we get a matching id
        loop {
            let mut line = String::new();
            self.reader
                .read_line(&mut line)
                .map_err(|e| ClawzError::Tool(format!("MCP read error: {e}")))?;

            if line.is_empty() {
                return Err(ClawzError::Tool("MCP server closed connection".into()));
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let resp: JsonRpcResponse = serde_json::from_str(line)
                .map_err(|e| ClawzError::Tool(format!("MCP JSON parse error: {e} — line: {line}")))?;

            // Check if this is our response
            if resp.id.as_u64() == Some(id) || resp.id.as_str() == Some(&id.to_string()) {
                if let Some(err) = resp.error {
                    return Err(ClawzError::Tool(format!(
                        "MCP error {}: {}",
                        err.code, err.message
                    )));
                }
                return Ok(resp.result.unwrap_or(Value::Null));
            }
            // Otherwise it might be a notification — ignore
        }
    }

    fn initialize(&mut self, client_name: &str) -> Result<McpServerInfo, ClawzError> {
        if self.initialized {
            return Err(ClawzError::Tool("already initialized".into()));
        }

        let result = self.send_request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "roots": { "listChanged": false },
                    "sampling": {}
                },
                "clientInfo": {
                    "name": client_name,
                    "version": "1.0.0"
                }
            }),
        )?;

        // Send initialized notification
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        writeln!(self.stdin, "{}", notif)
            .map_err(|e| ClawzError::Tool(format!("MCP notify error: {e}")))?;
        self.stdin.flush().ok();

        self.initialized = true;

        let info = McpServerInfo {
            name: result["serverInfo"]["name"]
                .as_str()
                .unwrap_or("unknown")
                .into(),
            version: result["serverInfo"]["version"]
                .as_str()
                .unwrap_or("0.0.0")
                .into(),
            capabilities: McpCapabilities {
                tools: result["capabilities"]["tools"].is_object(),
                resources: result["capabilities"]["resources"].is_object(),
                prompts: result["capabilities"]["prompts"].is_object(),
                sampling: result["capabilities"]["sampling"].is_object(),
            },
        };

        Ok(info)
    }

    fn list_tools(&mut self) -> Result<Vec<ToolSchema>, ClawzError> {
        let result = self.send_request("tools/list", serde_json::json!({}))?;

        let tools = result["tools"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|t| ToolSchema {
                name: t["name"].as_str().unwrap_or("").into(),
                description: t["description"].as_str().unwrap_or("").into(),
                parameters: t["inputSchema"].clone(),
            })
            .collect();

        Ok(tools)
    }

    fn call_tool(&mut self, tool_name: &str, args: Value) -> Result<Value, ClawzError> {
        self.send_request(
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": args
            }),
        )
    }

    fn list_resources(&mut self) -> Result<Vec<McpResource>, ClawzError> {
        let result = self.send_request("resources/list", serde_json::json!({}))?;

        let resources = result["resources"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|r| McpResource {
                uri: r["uri"].as_str().unwrap_or("").into(),
                name: r["name"].as_str().unwrap_or("").into(),
                description: r["description"].as_str().map(String::from),
                mime_type: r["mimeType"].as_str().map(String::from),
            })
            .collect();

        Ok(resources)
    }

    fn read_resource(&mut self, uri: &str) -> Result<String, ClawzError> {
        let result = self.send_request(
            "resources/read",
            serde_json::json!({ "uri": uri }),
        )?;

        // MCP returns contents as array of {type, text/blob}
        let content = result["contents"]
            .as_array()
            .unwrap_or(&vec![])
            .first()
            .map(|c| {
                c["text"]
                    .as_str()
                    .unwrap_or_else(|| c["blob"].as_str().unwrap_or(""))
                    .to_string()
            })
            .unwrap_or_default();

        Ok(content)
    }

    fn list_prompts(&mut self) -> Result<Vec<McpPrompt>, ClawzError> {
        let result = self.send_request("prompts/list", serde_json::json!({}))?;

        let prompts = result["prompts"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|p| McpPrompt {
                name: p["name"].as_str().unwrap_or("").into(),
                description: p["description"].as_str().map(String::from),
                arguments: p["arguments"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|a| McpPromptArg {
                        name: a["name"].as_str().unwrap_or("").into(),
                        description: a["description"].as_str().map(String::from),
                        required: a["required"].as_bool().unwrap_or(false),
                    })
                    .collect(),
            })
            .collect();

        Ok(prompts)
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        self.process.kill().ok();
    }
}

// ── HTTP MCP Client ────────────────────────────────────────────────────────────

struct HttpMcpClient {
    base_url: String,
    headers: HashMap<String, String>,
    http: reqwest::Client,
    next_id: u64,
}

impl HttpMcpClient {
    fn new(base_url: String, headers: HashMap<String, String>) -> Result<Self, ClawzError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| ClawzError::Tool(format!("HTTP client failed: {e}")))?;

        Ok(Self {
            base_url,
            headers,
            http,
            next_id: 1,
        })
    }

    async fn send_request(&mut self, method: &str, params: Value) -> Result<Value, ClawzError> {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest::new(id, method, params);

        let mut builder = self
            .http
            .post(format!("{}/mcp", self.base_url.trim_end_matches('/')))
            .json(&req);

        for (k, v) in &self.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        let resp: JsonRpcResponse = builder
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("HTTP MCP request failed: {e}")))?
            .json()
            .await
            .map_err(|e| ClawzError::Tool(format!("HTTP MCP response parse failed: {e}")))?;

        if let Some(err) = resp.error {
            return Err(ClawzError::Tool(format!(
                "MCP error {}: {}",
                err.code, err.message
            )));
        }

        Ok(resp.result.unwrap_or(Value::Null))
    }
}

// ── Public MCP Client ─────────────────────────────────────────────────────────

/// MCP client that can communicate via stdio or HTTP.
pub struct McpClient {
    transport: McpTransport,
}

impl McpClient {
    pub fn new_stdio(command: String, args: Vec<String>, env: HashMap<String, String>) -> Self {
        Self {
            transport: McpTransport::Stdio { command, args, env },
        }
    }

    pub fn new_http(base_url: String, headers: HashMap<String, String>) -> Self {
        Self {
            transport: McpTransport::Http { base_url, headers },
        }
    }

    /// Discover all tools exposed by the MCP server.
    pub async fn discover_tools(&self) -> Result<Vec<ToolSchema>, ClawzError> {
        match &self.transport {
            McpTransport::Stdio { command, args, env } => {
                let mut client = StdioMcpClient::spawn(command, args, env)?;
                client.initialize("clawz-worker")?;
                client.list_tools()
            }
            McpTransport::Http { base_url, headers } => {
                let mut client = HttpMcpClient::new(base_url.clone(), headers.clone())?;
                let result = client.send_request("tools/list", serde_json::json!({})).await?;
                let tools = result["tools"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|t| ToolSchema {
                        name: t["name"].as_str().unwrap_or("").into(),
                        description: t["description"].as_str().unwrap_or("").into(),
                        parameters: t["inputSchema"].clone(),
                    })
                    .collect();
                Ok(tools)
            }
        }
    }

    /// Call a tool on the MCP server.
    pub async fn execute_tool(&self, tool_name: &str, args: Value) -> Result<Value, ClawzError> {
        match &self.transport {
            McpTransport::Stdio { command, args: cmd_args, env } => {
                let mut client = StdioMcpClient::spawn(command, cmd_args, env)?;
                client.initialize("clawz-worker")?;
                client.call_tool(tool_name, args)
            }
            McpTransport::Http { base_url, headers } => {
                let mut client = HttpMcpClient::new(base_url.clone(), headers.clone())?;
                client
                    .send_request(
                        "tools/call",
                        serde_json::json!({ "name": tool_name, "arguments": args }),
                    )
                    .await
            }
        }
    }

    /// List resources from the MCP server.
    pub async fn list_resources(&self) -> Result<Vec<McpResource>, ClawzError> {
        match &self.transport {
            McpTransport::Stdio { command, args, env } => {
                let mut client = StdioMcpClient::spawn(command, args, env)?;
                client.initialize("clawz-worker")?;
                client.list_resources()
            }
            McpTransport::Http { base_url, headers } => {
                let mut client = HttpMcpClient::new(base_url.clone(), headers.clone())?;
                let result = client.send_request("resources/list", serde_json::json!({})).await?;
                let resources = result["resources"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|r| McpResource {
                        uri: r["uri"].as_str().unwrap_or("").into(),
                        name: r["name"].as_str().unwrap_or("").into(),
                        description: r["description"].as_str().map(String::from),
                        mime_type: r["mimeType"].as_str().map(String::from),
                    })
                    .collect();
                Ok(resources)
            }
        }
    }

    /// Read a resource from the MCP server.
    pub async fn read_resource(&self, uri: &str) -> Result<String, ClawzError> {
        match &self.transport {
            McpTransport::Stdio { command, args, env } => {
                let mut client = StdioMcpClient::spawn(command, args, env)?;
                client.initialize("clawz-worker")?;
                client.read_resource(uri)
            }
            McpTransport::Http { base_url, headers } => {
                let mut client = HttpMcpClient::new(base_url.clone(), headers.clone())?;
                let result = client
                    .send_request("resources/read", serde_json::json!({ "uri": uri }))
                    .await?;
                Ok(result["contents"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string())
            }
        }
    }

    /// List prompts from the MCP server.
    pub async fn list_prompts(&self) -> Result<Vec<McpPrompt>, ClawzError> {
        match &self.transport {
            McpTransport::Stdio { command, args, env } => {
                let mut client = StdioMcpClient::spawn(command, args, env)?;
                client.initialize("clawz-worker")?;
                client.list_prompts()
            }
            McpTransport::Http { base_url, headers } => {
                let mut client = HttpMcpClient::new(base_url.clone(), headers.clone())?;
                let result = client.send_request("prompts/list", serde_json::json!({})).await?;
                let prompts = result["prompts"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|p| McpPrompt {
                        name: p["name"].as_str().unwrap_or("").into(),
                        description: p["description"].as_str().map(String::from),
                        arguments: Vec::new(),
                    })
                    .collect();
                Ok(prompts)
            }
        }
    }
}

// ── MCP Server Registry Entry ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: McpServerEntryTransport,
    pub tools: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpServerEntryTransport {
    Stdio {
        command: String,
        args: Vec<String>,
    },
    Http {
        url: String,
    },
}

// ── MCP Server Manager ────────────────────────────────────────────────────────

/// Manages multiple MCP servers and the tools they provide.
pub struct McpServerManager {
    servers: Arc<Mutex<HashMap<String, McpServerEntry>>>,
    /// Track which tool names belong to which server
    tool_index: Arc<Mutex<HashMap<String, String>>>,
}

impl McpServerManager {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(Mutex::new(HashMap::new())),
            tool_index: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register an MCP server and discover its tools.
    pub async fn register_server(&self, entry: McpServerEntry) -> Result<Vec<ToolSchema>, ClawzError> {
        let client = match &entry.transport {
            McpServerEntryTransport::Stdio { command, args } => {
                McpClient::new_stdio(command.clone(), args.clone(), HashMap::new())
            }
            McpServerEntryTransport::Http { url } => {
                McpClient::new_http(url.clone(), HashMap::new())
            }
        };

        let tools = client.discover_tools().await?;

        // Index tools -> server
        {
            let mut idx = self.tool_index.lock().await;
            for tool in &tools {
                idx.insert(tool.name.clone(), entry.name.clone());
            }
        }

        let mut servers = self.servers.lock().await;
        let mut updated_entry = entry;
        updated_entry.tools = tools.iter().map(|t| t.name.clone()).collect();
        servers.insert(updated_entry.name.clone(), updated_entry);

        Ok(tools)
    }

    /// Unregister a server.
    pub async fn unregister_server(&self, name: &str) {
        let mut servers = self.servers.lock().await;
        if let Some(entry) = servers.remove(name) {
            let mut idx = self.tool_index.lock().await;
            for tool in &entry.tools {
                idx.remove(tool);
            }
        }
    }

    /// List all registered servers.
    pub async fn list_servers(&self) -> Vec<McpServerEntry> {
        self.servers.lock().await.values().cloned().collect()
    }

    /// Find which server provides a given tool.
    pub async fn server_for_tool(&self, tool_name: &str) -> Option<McpServerEntry> {
        let idx = self.tool_index.lock().await;
        let server_name = idx.get(tool_name)?.clone();
        drop(idx);
        self.servers.lock().await.get(&server_name).cloned()
    }

    /// Execute a tool by routing to the appropriate server.
    pub async fn execute_tool(&self, tool_name: &str, args: Value) -> Result<Value, ClawzError> {
        let server = self.server_for_tool(tool_name).await.ok_or_else(|| {
            ClawzError::NotFound {
                entity: "MCP tool".into(),
                id: tool_name.into(),
            }
        })?;

        let client = match &server.transport {
            McpServerEntryTransport::Stdio { command, args: cmd_args } => {
                McpClient::new_stdio(command.clone(), cmd_args.clone(), HashMap::new())
            }
            McpServerEntryTransport::Http { url } => {
                McpClient::new_http(url.clone(), HashMap::new())
            }
        };

        client.execute_tool(tool_name, args).await
    }

    /// Get all tool schemas from all registered servers.
    pub async fn all_tools(&self) -> Result<Vec<ToolSchema>, ClawzError> {
        let servers: Vec<McpServerEntry> = self.servers.lock().await.values().cloned().collect();
        let mut all = Vec::new();

        for server in servers {
            if !server.enabled {
                continue;
            }
            let client = match &server.transport {
                McpServerEntryTransport::Stdio { command, args } => {
                    McpClient::new_stdio(command.clone(), args.clone(), HashMap::new())
                }
                McpServerEntryTransport::Http { url } => {
                    McpClient::new_http(url.clone(), HashMap::new())
                }
            };
            match client.discover_tools().await {
                Ok(tools) => all.extend(tools),
                Err(e) => log::warn!("failed to list tools from {}: {e}", server.name),
            }
        }

        Ok(all)
    }
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── McpTool — wraps an MCP server tool as a local Tool ────────────────────────

use crate::tools::tool_trait::{Tool, ToolContext};
use async_trait::async_trait;
use clawz_core::types::ToolResult;

/// Wraps an MCP server tool as a clawz-worker Tool.
pub struct McpTool {
    schema: ToolSchema,
    transport: McpTransport,
}

impl McpTool {
    pub fn new(schema: ToolSchema, transport: McpTransport) -> Self {
        Self { schema, transport }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.schema.name
    }

    fn description(&self) -> &str {
        &self.schema.description
    }

    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult, ClawzError> {
        let client = match &self.transport {
            McpTransport::Stdio { command, args: cmd_args, env } => {
                McpClient::new_stdio(command.clone(), cmd_args.clone(), env.clone())
            }
            McpTransport::Http { base_url, headers } => {
                McpClient::new_http(base_url.clone(), headers.clone())
            }
        };

        let result = client.execute_tool(&self.schema.name, args).await?;

        // Extract text from MCP content array
        let output = if let Some(arr) = result["content"].as_array() {
            arr.iter()
                .filter_map(|c| {
                    if c["type"].as_str() == Some("text") {
                        c["text"].as_str().map(String::from)
                    } else {
                        Some(c.to_string())
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            result.to_string()
        };

        let is_error = result["isError"].as_bool().unwrap_or(false);

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest::new(1, "tools/list", serde_json::json!({}));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"tools/list\""));
    }

    #[test]
    fn test_json_rpc_response_with_error() {
        let resp_json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(resp_json).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn test_mcp_server_manager_creation() {
        let mgr = McpServerManager::new();
        let _ = mgr; // just test it compiles
    }

    #[test]
    fn test_mcp_client_http_creation() {
        let client = McpClient::new_http(
            "http://localhost:8080".into(),
            HashMap::new(),
        );
        let _ = client;
    }

    #[test]
    fn test_mcp_client_stdio_creation() {
        let client = McpClient::new_stdio(
            "mcp-server".into(),
            vec!["--stdio".into()],
            HashMap::new(),
        );
        let _ = client;
    }

    #[tokio::test]
    async fn test_manager_list_servers_empty() {
        let mgr = McpServerManager::new();
        let servers = mgr.list_servers().await;
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn test_manager_server_for_nonexistent_tool() {
        let mgr = McpServerManager::new();
        let result = mgr.server_for_tool("nonexistent_tool").await;
        assert!(result.is_none());
    }

    #[test]
    fn test_mcp_resource_serialization() {
        let resource = McpResource {
            uri: "file:///test.txt".into(),
            name: "test".into(),
            description: Some("A test file".into()),
            mime_type: Some("text/plain".into()),
        };
        let json = serde_json::to_string(&resource).unwrap();
        assert!(json.contains("file:///test.txt"));
    }
}
