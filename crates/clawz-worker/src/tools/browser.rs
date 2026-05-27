//! Browser automation tool using Chrome DevTools Protocol (CDP).
//!
//! This module implements [`BrowserManager`], which controls a Chrome/Chromium
//! instance over the DevTools Protocol, enabling agents to navigate, read content,
//! click elements, and interact with web pages programmatically.
//!
//! ## Features
//!
//! - **Browser launch**: Start a Chrome instance with configurable flags and ports
//! - **Tab management**: Open, close, and switch between browser tabs
//! - **Navigation**: Load URLs and wait for network idle
//! - **Content extraction**: Read DOM, get page title, extract text
//! - **Interaction**: Click elements, type text, scroll, submit forms
//! - **Screenshots**: Capture page screenshots for visual inspection
//!
//! ## Cross-module dependencies
//!
//! // Dependency: Uses Chrome/Chromium binary via CDP over WebSocket.
//! // Dependency: Implements the [`Tool`] trait from [`super::tool_trait`].

use clawz_core::error::ClawzError;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

/// Configuration for launching a browser instance.
#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// CDP debug port (default: 9222)
    pub debug_port: u16,
    /// Path to Chrome/Chromium binary
    pub chrome_path: Option<String>,
    /// Additional command-line flags
    pub extra_flags: Vec<String>,
    /// Window size
    pub window_width: u32,
    pub window_height: u32,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            debug_port: 9222,
            chrome_path: None,
            extra_flags: Vec::new(),
            window_width: 1280,
            window_height: 720,
        }
    }
}

/// Represents a browser tab (CDP target).
#[derive(Debug, Clone)]
pub struct BrowserTab {
    pub id: String,
    pub url: String,
    pub title: String,
    pub ws_url: String,
}

/// Page pool entry — a held WebSocket connection to a tab.
type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

struct TabConnection {
    ws: WsStream,
    next_id: u64,
}

impl TabConnection {
    async fn send(&mut self, method: &str, params: Value) -> Result<Value, ClawzError> {
        let id = self.next_id;
        self.next_id += 1;

        let cmd = serde_json::json!({
            "id": id,
            "method": method,
            "params": params
        });

        self.ws
            .send(Message::Text(cmd.to_string()))
            .await
            .map_err(|e| ClawzError::Tool(format!("CDP send failed: {e}")))?;

        // Read until we get the matching response id
        loop {
            let msg = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.ws.next(),
            )
            .await
            .map_err(|_| ClawzError::Tool("CDP response timeout".into()))?
            .ok_or_else(|| ClawzError::Tool("CDP WebSocket closed".into()))?
            .map_err(|e| ClawzError::Tool(format!("CDP WebSocket error: {e}")))?;

            if let Message::Text(text) = msg {
                let val: Value = serde_json::from_str(&text)
                    .map_err(|e| ClawzError::Tool(format!("CDP JSON error: {e}")))?;

                if val["id"].as_u64() == Some(id) {
                    if let Some(err_obj) = val["error"].as_object() {
                        return Err(ClawzError::Tool(format!(
                            "CDP error: {}",
                            err_obj["message"].as_str().unwrap_or("unknown")
                        )));
                    }
                    return Ok(val["result"].clone());
                }
                // Ignore events
            }
        }
    }
}

/// Manages a pool of browser tabs for parallel scraping/automation.
pub struct BrowserManager {
    config: BrowserConfig,
    /// CDP base URL (http://host:port)
    cdp_base: String,
    /// Active tab connections keyed by tab ID
    connections: Arc<Mutex<HashMap<String, Arc<Mutex<TabConnection>>>>>,
    /// Chrome process handle (if we launched it)
    process: Arc<Mutex<Option<tokio::process::Child>>>,
}

impl BrowserManager {
    /// Create a manager that connects to an existing Chrome instance.
    pub fn new(config: BrowserConfig) -> Self {
        let cdp_base = std::env::var("CHROME_CDP_URL")
            .unwrap_or_else(|_| format!("http://localhost:{}", config.debug_port));

        Self {
            config,
            cdp_base,
            connections: Arc::new(Mutex::new(HashMap::new())),
            process: Arc::new(Mutex::new(None)),
        }
    }

    pub fn default_config() -> Self {
        Self::new(BrowserConfig::default())
    }

    /// Launch a headless Chrome/Chromium process.
    pub async fn launch(&self) -> Result<(), ClawzError> {
        let chrome_bin = self
            .config
            .chrome_path
            .clone()
            .or_else(|| std::env::var("CHROME_BIN").ok())
            .unwrap_or_else(find_chrome_binary);

        let mut cmd = tokio::process::Command::new(&chrome_bin);
        cmd.args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--disable-setuid-sandbox",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-sync",
        ])
        .arg(format!("--remote-debugging-port={}", self.config.debug_port))
        .arg(format!(
            "--window-size={},{}",
            self.config.window_width, self.config.window_height
        ));

        for flag in &self.config.extra_flags {
            cmd.arg(flag);
        }

        let child = cmd
            .spawn()
            .map_err(|e| ClawzError::Tool(format!("failed to launch Chrome '{}': {e}", chrome_bin)))?;

        *self.process.lock().await = Some(child);

        // Wait for CDP to become available
        for attempt in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if self.is_available().await {
                return Ok(());
            }
            if attempt == 19 {
                return Err(ClawzError::Tool("Chrome CDP did not become available after 4s".into()));
            }
        }

        Ok(())
    }

    /// Check if the CDP endpoint is reachable.
    pub async fn is_available(&self) -> bool {
        let client = reqwest::Client::new();
        client
            .get(format!("{}/json/version", self.cdp_base))
            .timeout(std::time::Duration::from_millis(500))
            .send()
            .await
            .is_ok()
    }

    /// List all open tabs.
    pub async fn list_tabs(&self) -> Result<Vec<BrowserTab>, ClawzError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| ClawzError::Tool(format!("HTTP client failed: {e}")))?;

        let targets: Vec<Value> = client
            .get(format!("{}/json/list", self.cdp_base))
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("CDP list request failed: {e}")))?
            .json()
            .await
            .map_err(|e| ClawzError::Tool(format!("CDP list parse failed: {e}")))?;

        let tabs = targets
            .iter()
            .filter(|t| t["type"].as_str() == Some("page"))
            .map(|t| BrowserTab {
                id: t["id"].as_str().unwrap_or("").to_string(),
                url: t["url"].as_str().unwrap_or("").to_string(),
                title: t["title"].as_str().unwrap_or("").to_string(),
                ws_url: t["webSocketDebuggerUrl"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            })
            .collect();

        Ok(tabs)
    }

    /// Open a new tab and return its ID.
    pub async fn new_tab(&self) -> Result<String, ClawzError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| ClawzError::Tool(format!("HTTP client failed: {e}")))?;

        let target: Value = client
            .put(format!("{}/json/new", self.cdp_base))
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("new tab request failed: {e}")))?
            .json()
            .await
            .map_err(|e| ClawzError::Tool(format!("new tab response parse failed: {e}")))?;

        let tab_id = target["id"]
            .as_str()
            .ok_or_else(|| ClawzError::Tool("no tab id in response".into()))?
            .to_string();

        Ok(tab_id)
    }

    /// Close a tab by ID.
    pub async fn close_tab(&self, tab_id: &str) -> Result<(), ClawzError> {
        // Remove from connection pool
        self.connections.lock().await.remove(tab_id);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| ClawzError::Tool(format!("HTTP client failed: {e}")))?;

        client
            .get(format!("{}/json/close/{}", self.cdp_base, tab_id))
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("close tab request failed: {e}")))?;

        Ok(())
    }

    /// Get or create a connection to a specific tab.
    async fn get_connection(
        &self,
        tab_id: &str,
        ws_url: &str,
    ) -> Result<Arc<Mutex<TabConnection>>, ClawzError> {
        let mut conns = self.connections.lock().await;
        if let Some(conn) = conns.get(tab_id) {
            return Ok(conn.clone());
        }

        // Connect
        let (ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| ClawzError::Tool(format!("WebSocket connect failed: {e}")))?;

        let conn = Arc::new(Mutex::new(TabConnection { ws, next_id: 1 }));
        conns.insert(tab_id.to_string(), conn.clone());
        Ok(conn)
    }

    /// Get connection for first available tab, creating one if needed.
    async fn get_any_connection(&self) -> Result<(String, Arc<Mutex<TabConnection>>), ClawzError> {
        let tabs = self.list_tabs().await?;

        let tab = if let Some(t) = tabs.first() {
            t.clone()
        } else {
            // No tabs — create one
            let new_id = self.new_tab().await?;
            // Refresh list
            let tabs2 = self.list_tabs().await?;
            tabs2
                .into_iter()
                .find(|t| t.id == new_id)
                .ok_or_else(|| ClawzError::Tool("newly created tab not found".into()))?
        };

        let conn = self.get_connection(&tab.id, &tab.ws_url).await?;
        Ok((tab.id, conn))
    }

    /// Navigate to a URL.
    pub async fn navigate(&self, url: &str) -> Result<String, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        let result = c
            .send("Page.navigate", serde_json::json!({ "url": url }))
            .await?;

        let frame_id = result["frameId"].as_str().unwrap_or("").to_string();

        // Wait for load
        drop(c);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        Ok(frame_id)
    }

    /// Get the visible text content of the current page.
    pub async fn get_content(&self) -> Result<String, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "document.body.innerText",
                    "returnByValue": true
                }),
            )
            .await?;

        Ok(result["result"]["value"].as_str().unwrap_or("").to_string())
    }

    /// Get the full page HTML.
    pub async fn get_html(&self) -> Result<String, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "document.documentElement.outerHTML",
                    "returnByValue": true
                }),
            )
            .await?;

        Ok(result["result"]["value"].as_str().unwrap_or("").to_string())
    }

    /// Take a screenshot of the current page. Returns PNG bytes.
    pub async fn screenshot(&self) -> Result<Vec<u8>, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        let result = c
            .send(
                "Page.captureScreenshot",
                serde_json::json!({ "format": "png", "quality": 100 }),
            )
            .await?;

        let b64 = result["data"]
            .as_str()
            .ok_or_else(|| ClawzError::Tool("no screenshot data in response".into()))?;

        use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
        B64.decode(b64)
            .map_err(|e| ClawzError::Tool(format!("screenshot decode failed: {e}")))
    }

    /// Click an element matching the CSS selector.
    pub async fn click(&self, selector: &str) -> Result<(), ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;

        let script = format!(
            r#"(() => {{
                const el = document.querySelector('{}');
                if (!el) return null;
                const r = el.getBoundingClientRect();
                return {{ x: r.left + r.width/2, y: r.top + r.height/2 }};
            }})()"#,
            selector.replace('\'', "\\'")
        );

        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({ "expression": script, "returnByValue": true }),
            )
            .await?;

        let coords = &result["result"]["value"];
        let x = coords["x"]
            .as_f64()
            .ok_or_else(|| ClawzError::Tool(format!("element '{}' not found", selector)))?;
        let y = coords["y"]
            .as_f64()
            .ok_or_else(|| ClawzError::Tool("no y coordinate".into()))?;

        for event_type in ["mousePressed", "mouseReleased"] {
            c.send(
                "Input.dispatchMouseEvent",
                serde_json::json!({
                    "type": event_type,
                    "x": x,
                    "y": y,
                    "button": "left",
                    "clickCount": 1
                }),
            )
            .await?;
        }

        Ok(())
    }

    /// Type text into an element matching the CSS selector.
    pub async fn type_text(&self, selector: &str, text: &str) -> Result<(), ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;

        // Focus the element
        let focus_script = format!(
            "document.querySelector('{}').focus()",
            selector.replace('\'', "\\'")
        );
        c.send(
            "Runtime.evaluate",
            serde_json::json!({ "expression": focus_script }),
        )
        .await?;

        // Insert text
        c.send("Input.insertText", serde_json::json!({ "text": text }))
            .await?;

        Ok(())
    }

    /// Evaluate JavaScript in the page context.
    pub async fn evaluate_js(&self, script: &str) -> Result<Value, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;

        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": script,
                    "returnByValue": true,
                    "awaitPromise": true
                }),
            )
            .await?;

        Ok(result["result"]["value"].clone())
    }

    /// Wait for an element matching the CSS selector to appear.
    pub async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<(), ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;

        let script = format!(
            r#"new Promise((resolve, reject) => {{
                const start = Date.now();
                const check = setInterval(() => {{
                    if (document.querySelector('{}')) {{
                        clearInterval(check);
                        resolve(true);
                    }} else if (Date.now() - start > {}) {{
                        clearInterval(check);
                        reject(new Error('wait_for timeout'));
                    }}
                }}, 100);
            }})"#,
            selector.replace('\'', "\\'"),
            timeout_ms
        );

        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": script,
                    "awaitPromise": true,
                    "returnByValue": true
                }),
            )
            .await?;

        // Check for exception
        if result["exceptionDetails"].is_object() {
            return Err(ClawzError::Tool(format!(
                "wait_for '{}' timed out after {}ms",
                selector, timeout_ms
            )));
        }

        Ok(())
    }

    /// Extract all links from the current page.
    pub async fn get_links(&self) -> Result<Vec<String>, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;

        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "Array.from(document.querySelectorAll('a[href]')).map(a => a.href)",
                    "returnByValue": true
                }),
            )
            .await?;

        let links = result["result"]["value"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        Ok(links)
    }

    /// Fill form fields. `fields` maps CSS selector → value.
    pub async fn fill_form(&self, fields: HashMap<String, String>) -> Result<usize, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        let mut filled = 0usize;

        for (selector, value) in &fields {
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
                value.replace('\'', "\\'")
            );

            let result = c
                .send(
                    "Runtime.evaluate",
                    serde_json::json!({ "expression": script, "returnByValue": true }),
                )
                .await?;

            if result["result"]["value"].as_bool() == Some(true) {
                filled += 1;
            }
        }

        Ok(filled)
    }

    /// Get the current page URL.
    pub async fn current_url(&self) -> Result<String, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "location.href",
                    "returnByValue": true
                }),
            )
            .await?;
        Ok(result["result"]["value"].as_str().unwrap_or("").to_string())
    }

    /// Get the page title.
    pub async fn get_title(&self) -> Result<String, ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        let result = c
            .send(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "document.title",
                    "returnByValue": true
                }),
            )
            .await?;
        Ok(result["result"]["value"].as_str().unwrap_or("").to_string())
    }

    /// Scroll the page.
    pub async fn scroll(&self, x: f64, y: f64) -> Result<(), ClawzError> {
        let (_, conn) = self.get_any_connection().await?;
        let mut c = conn.lock().await;
        c.send(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": format!("window.scrollBy({}, {})", x, y)
            }),
        )
        .await?;
        Ok(())
    }

    /// Stop the Chrome process if we launched it.
    pub async fn shutdown(&self) -> Result<(), ClawzError> {
        // Close all connections
        self.connections.lock().await.clear();

        // Kill the Chrome process
        if let Some(mut child) = self.process.lock().await.take() {
            child.kill().await.ok();
        }

        Ok(())
    }
}

fn find_chrome_binary() -> String {
    let candidates = [
        "chromium-browser",
        "chromium",
        "google-chrome",
        "google-chrome-stable",
        "google-chrome-beta",
        "/usr/bin/chromium-browser",
        "/usr/bin/chromium",
        "/usr/bin/google-chrome",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];

    for candidate in &candidates {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
        // Also check PATH
        if std::process::Command::new("which")
            .arg(candidate)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return candidate.to_string();
        }
    }

    "chromium".to_string() // best guess
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_manager_creation() {
        let mgr = BrowserManager::default_config();
        assert!(mgr.cdp_base.contains("localhost") || mgr.cdp_base.contains("http"));
    }

    #[test]
    fn test_browser_config_default() {
        let cfg = BrowserConfig::default();
        assert_eq!(cfg.debug_port, 9222);
        assert_eq!(cfg.window_width, 1280);
        assert_eq!(cfg.window_height, 720);
    }

    #[tokio::test]
    async fn test_is_available_no_chrome() {
        // Without Chrome running, should return false (not panic)
        let mgr = BrowserManager::default_config();
        let available = mgr.is_available().await;
        // Just check it doesn't panic — result depends on environment
        let _ = available;
    }

    #[test]
    fn test_find_chrome_binary_doesnt_panic() {
        let _ = find_chrome_binary();
    }
}
