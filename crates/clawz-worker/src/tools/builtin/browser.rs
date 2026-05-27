use crate::tools::tool_trait::{Tool, ToolContext};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use clawz_core::types::{ToolResult, ToolSchema};
use serde_json::Value;

/// Built-in browser tool that delegates to the BrowserManager.
/// For direct CDP access use `crate::tools::browser::BrowserManager`.
pub struct BrowserTool;

impl BrowserTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Control a headless Chromium browser via Chrome DevTools Protocol. Supports navigation, screenshots, clicking, typing, JS evaluation, and link extraction."
    }

    fn primitive(&self) -> ActionPrimitive {
        ActionPrimitive::Execute
    }
    fn risk(&self) -> RiskLevel {
        RiskLevel::High
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["navigate", "screenshot", "get_content", "click",
                                 "type_text", "evaluate_js", "wait_for",
                                 "get_links", "fill_form"],
                        "description": "Browser action to perform"
                    },
                    "url": {
                        "type": "string",
                        "description": "URL to navigate to (for 'navigate' action)"
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector (for click/type_text/wait_for)"
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to type (for 'type_text' action)"
                    },
                    "script": {
                        "type": "string",
                        "description": "JavaScript to evaluate (for 'evaluate_js' action)"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds for wait_for (default: 5000)"
                    },
                    "fields": {
                        "type": "object",
                        "description": "Key-value map of selector→value for fill_form",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult, ClawzError> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("action required".into()))?;

        // Attempt to connect to a running Chrome instance on default debug port
        execute_cdp_action(action, &args).await
    }
}

/// Execute a CDP action against a headless Chrome instance.
/// Chrome must be started with --remote-debugging-port=9222
async fn execute_cdp_action(action: &str, args: &Value) -> Result<ToolResult, ClawzError> {
    use futures_util::SinkExt;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;

    // Get the Chrome DevTools endpoint
    let cdp_base =
        std::env::var("CHROME_CDP_URL").unwrap_or_else(|_| "http://localhost:9222".into());

    // Fetch the list of targets to get a WebSocket debugger URL
    let client = reqwest::Client::new();
    let targets_url = format!("{}/json/list", cdp_base);
    let targets_resp = client
        .get(&targets_url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| ClawzError::Tool(format!("Chrome CDP not available: {e}")))?;

    let targets: Vec<Value> = targets_resp
        .json()
        .await
        .map_err(|e| ClawzError::Tool(format!("failed to parse CDP targets: {e}")))?;

    // Use the first page target or create a new one
    let ws_url = if let Some(target) = targets.iter().find(|t| t["type"].as_str() == Some("page")) {
        target["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| ClawzError::Tool("no webSocketDebuggerUrl".into()))?
            .to_string()
    } else {
        // Create a new target
        let new_target: Value = client
            .put(format!("{}/json/new", cdp_base))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to create CDP target: {e}")))?
            .json()
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to parse new target: {e}")))?;

        new_target["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| ClawzError::Tool("no webSocketDebuggerUrl on new target".into()))?
            .to_string()
    };

    // Connect via WebSocket
    let (mut ws, _) = connect_async(&ws_url)
        .await
        .map_err(|e| ClawzError::Tool(format!("WebSocket connect failed: {e}")))?;

    let output = match action {
        "navigate" => {
            let url = args["url"]
                .as_str()
                .ok_or_else(|| ClawzError::Validation("url required for navigate".into()))?;

            let cmd = serde_json::json!({
                "id": 1,
                "method": "Page.navigate",
                "params": { "url": url }
            });
            ws.send(Message::Text(cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send error: {e}")))?;

            let resp = recv_cdp_result(&mut ws, 1).await?;
            format!(
                "navigated to {} — frameId: {}",
                url,
                resp["result"]["frameId"].as_str().unwrap_or("unknown")
            )
        }

        "get_content" => {
            // Use Runtime.evaluate to get document body text
            let cmd = serde_json::json!({
                "id": 2,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": "document.body.innerText",
                    "returnByValue": true
                }
            });
            ws.send(Message::Text(cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send error: {e}")))?;
            let resp = recv_cdp_result(&mut ws, 2).await?;
            resp["result"]["result"]["value"]
                .as_str()
                .unwrap_or("")
                .to_string()
        }

        "screenshot" => {
            let cmd = serde_json::json!({
                "id": 3,
                "method": "Page.captureScreenshot",
                "params": { "format": "png" }
            });
            ws.send(Message::Text(cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send error: {e}")))?;
            let resp = recv_cdp_result(&mut ws, 3).await?;
            let data = resp["result"]["data"].as_str().unwrap_or("").to_string();
            format!("base64_png:{}", data)
        }

        "click" => {
            let selector = args["selector"]
                .as_str()
                .ok_or_else(|| ClawzError::Validation("selector required for click".into()))?;

            // Get element coordinates via DOM
            let find_cmd = serde_json::json!({
                "id": 10,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": format!(
                        "(() => {{ const el = document.querySelector('{}'); if (!el) return null; const r = el.getBoundingClientRect(); return {{x: r.left + r.width/2, y: r.top + r.height/2}}; }})()",
                        selector.replace('\'', "\\'")
                    ),
                    "returnByValue": true
                }
            });
            ws.send(Message::Text(find_cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
            let find_resp = recv_cdp_result(&mut ws, 10).await?;
            let coords = &find_resp["result"]["result"]["value"];
            let x = coords["x"]
                .as_f64()
                .ok_or_else(|| ClawzError::Tool(format!("selector '{}' not found", selector)))?;
            let y = coords["y"]
                .as_f64()
                .ok_or_else(|| ClawzError::Tool("element has no y coord".into()))?;

            // Mouse press + release
            for (event_type, id) in [("mousePressed", 11u64), ("mouseReleased", 12u64)] {
                let mouse_cmd = serde_json::json!({
                    "id": id,
                    "method": "Input.dispatchMouseEvent",
                    "params": {
                        "type": event_type,
                        "x": x,
                        "y": y,
                        "button": "left",
                        "clickCount": 1
                    }
                });
                ws.send(Message::Text(mouse_cmd.to_string()))
                    .await
                    .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
                recv_cdp_result(&mut ws, id).await?;
            }

            format!("clicked element '{}' at ({:.0}, {:.0})", selector, x, y)
        }

        "type_text" => {
            let selector = args["selector"]
                .as_str()
                .ok_or_else(|| ClawzError::Validation("selector required".into()))?;
            let text = args["text"]
                .as_str()
                .ok_or_else(|| ClawzError::Validation("text required".into()))?;

            // Focus the element first
            let focus_cmd = serde_json::json!({
                "id": 20,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": format!("document.querySelector('{}').focus()", selector.replace('\'', "\\'")),
                    "returnByValue": true
                }
            });
            ws.send(Message::Text(focus_cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
            recv_cdp_result(&mut ws, 20).await?;

            // Insert text
            let type_cmd = serde_json::json!({
                "id": 21,
                "method": "Input.insertText",
                "params": { "text": text }
            });
            ws.send(Message::Text(type_cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
            recv_cdp_result(&mut ws, 21).await?;

            format!("typed {} chars into '{}'", text.len(), selector)
        }

        "evaluate_js" => {
            let script = args["script"]
                .as_str()
                .ok_or_else(|| ClawzError::Validation("script required".into()))?;
            let cmd = serde_json::json!({
                "id": 30,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": script,
                    "returnByValue": true,
                    "awaitPromise": true
                }
            });
            ws.send(Message::Text(cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
            let resp = recv_cdp_result(&mut ws, 30).await?;
            resp["result"]["result"]["value"].to_string()
        }

        "wait_for" => {
            let selector = args["selector"]
                .as_str()
                .ok_or_else(|| ClawzError::Validation("selector required".into()))?;
            let timeout_ms = args["timeout_ms"].as_u64().unwrap_or(5000);

            // Poll for element existence
            let script = format!(
                r#"new Promise((resolve, reject) => {{
                    const start = Date.now();
                    const interval = setInterval(() => {{
                        if (document.querySelector('{}')) {{
                            clearInterval(interval);
                            resolve(true);
                        }} else if (Date.now() - start > {}) {{
                            clearInterval(interval);
                            reject(new Error('timeout'));
                        }}
                    }}, 100);
                }})"#,
                selector.replace('\'', "\\'"),
                timeout_ms
            );
            let cmd = serde_json::json!({
                "id": 40,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": script,
                    "returnByValue": true,
                    "awaitPromise": true
                }
            });
            ws.send(Message::Text(cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
            recv_cdp_result(&mut ws, 40).await?;
            format!("element '{}' appeared", selector)
        }

        "get_links" => {
            let cmd = serde_json::json!({
                "id": 50,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": "Array.from(document.querySelectorAll('a[href]')).map(a => a.href)",
                    "returnByValue": true
                }
            });
            ws.send(Message::Text(cmd.to_string()))
                .await
                .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
            let resp = recv_cdp_result(&mut ws, 50).await?;
            let links = &resp["result"]["result"]["value"];
            links.to_string()
        }

        "fill_form" => {
            let fields = args["fields"]
                .as_object()
                .ok_or_else(|| ClawzError::Validation("fields object required".into()))?;

            let mut filled = 0usize;
            for (selector, value) in fields {
                if let Some(text) = value.as_str() {
                    let script = format!(
                        r#"(() => {{
                            const el = document.querySelector('{}');
                            if (!el) return false;
                            el.focus();
                            el.value = '{}';
                            el.dispatchEvent(new Event('input', {{bubbles: true}}));
                            el.dispatchEvent(new Event('change', {{bubbles: true}}));
                            return true;
                        }})()"#,
                        selector.replace('\'', "\\'"),
                        text.replace('\'', "\\'")
                    );
                    let cmd = serde_json::json!({
                        "id": 60 + filled as u64,
                        "method": "Runtime.evaluate",
                        "params": { "expression": script, "returnByValue": true }
                    });
                    ws.send(Message::Text(cmd.to_string()))
                        .await
                        .map_err(|e| ClawzError::Tool(format!("CDP send: {e}")))?;
                    recv_cdp_result(&mut ws, 60 + filled as u64).await?;
                    filled += 1;
                }
            }
            format!("filled {} form fields", filled)
        }

        other => {
            return Err(ClawzError::Validation(format!("unknown action: {}", other)));
        }
    };

    ws.close(None).await.ok(); // best-effort close

    Ok(ToolResult {
        tool_call_id: String::new(),
        output,
        is_error: false,
    })
}

/// Wait for a CDP response with the given id.
async fn recv_cdp_result(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_id: u64,
) -> Result<Value, ClawzError> {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    // Read messages until we get a response with matching id
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(30), ws.next())
            .await
            .map_err(|_| ClawzError::Tool("CDP response timeout".into()))?
            .ok_or_else(|| ClawzError::Tool("CDP WebSocket closed".into()))?
            .map_err(|e| ClawzError::Tool(format!("CDP WebSocket error: {e}")))?;

        if let Message::Text(text) = msg {
            let val: Value = serde_json::from_str(&text)
                .map_err(|e| ClawzError::Tool(format!("CDP JSON parse error: {e}")))?;

            if val["id"].as_u64() == Some(expected_id) {
                if let Some(err) = val["error"].as_object() {
                    return Err(ClawzError::Tool(format!(
                        "CDP error: {}",
                        err["message"].as_str().unwrap_or("unknown")
                    )));
                }
                return Ok(val);
            }
            // Otherwise it's an event — ignore and continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolConfig;

    fn make_ctx() -> ToolContext {
        ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: ToolConfig::default(),
        }
    }

    #[test]
    fn test_browser_name() {
        let tool = BrowserTool::new();
        assert_eq!(tool.name(), "browser");
    }

    #[test]
    fn test_browser_schema() {
        let tool = BrowserTool::new();
        let schema = tool.schema();
        assert_eq!(schema.name, "browser");
        let actions = &schema.parameters["properties"]["action"]["enum"];
        assert!(
            actions
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("screenshot"))
        );
        assert!(
            actions
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("navigate"))
        );
        assert!(
            actions
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("evaluate_js"))
        );
    }

    #[tokio::test]
    async fn test_browser_missing_action() {
        let tool = BrowserTool::new();
        let ctx = make_ctx();
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_browser_unknown_action_schema_check() {
        // When no Chrome is available, we should get a Tool error (not Validation)
        // The "action required" check happens before CDP connection
        let tool = BrowserTool::new();
        let ctx = make_ctx();
        // With an unsupported action, the CDP connection attempt will fail or the action
        // matching will return Validation error
        let result = tool
            .execute(
                &ctx,
                serde_json::json!({"action": "navigate", "url": "https://example.com"}),
            )
            .await;
        // Either succeeds (Chrome running) or fails with Tool error (no Chrome)
        if let Err(e) = result {
            let msg = e.to_string();
            // Should be a tool/transport error, not a schema validation error
            assert!(
                msg.contains("CDP")
                    || msg.contains("Chrome")
                    || msg.contains("refused")
                    || msg.contains("not available")
                    || msg.contains("connect")
                    || msg.contains("tool"),
                "unexpected error: {}",
                msg
            );
        }
    }
}
