use crate::tools::tool_trait::{Tool, ToolContext};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use clawz_core::error::ClawzError;
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use clawz_core::types::{ToolResult, ToolSchema};
use std::net::IpAddr;
use std::str::FromStr;

const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024; // 10 MB

pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }

    fn is_private_ip(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                // 127.0.0.0/8 loopback
                if octets[0] == 127 {
                    return true;
                }
                // 10.0.0.0/8
                if octets[0] == 10 {
                    return true;
                }
                // 172.16.0.0/12
                if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                    return true;
                }
                // 192.168.0.0/16
                if octets[0] == 192 && octets[1] == 168 {
                    return true;
                }
                // 169.254.0.0/16 link-local
                if octets[0] == 169 && octets[1] == 254 {
                    return true;
                }
                // 0.0.0.0/8
                if octets[0] == 0 {
                    return true;
                }
                false
            }
            IpAddr::V6(v6) => {
                // ::1 loopback
                if v6.is_loopback() {
                    return true;
                }
                // fc00::/7 unique local
                let segments = v6.segments();
                if (segments[0] & 0xfe00) == 0xfc00 {
                    return true;
                }
                // fe80::/10 link-local
                if (segments[0] & 0xffc0) == 0xfe80 {
                    return true;
                }
                false
            }
        }
    }

    fn check_ssrf(url: &str) -> Result<(), ClawzError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| ClawzError::Validation(format!("invalid URL: {e}")))?;

        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(ClawzError::Validation(format!(
                "scheme '{scheme}' not allowed; only http/https"
            )));
        }

        if let Some(host) = parsed.host_str() {
            // Check if it's an IP address
            if let Ok(ip) = IpAddr::from_str(host) {
                if Self::is_private_ip(&ip) {
                    return Err(ClawzError::Validation(
                        "SSRF protection: private/loopback IP addresses are not allowed".into(),
                    ));
                }
            }
            // Block obvious internal hostnames
            let host_lower = host.to_lowercase();
            if host_lower == "localhost"
                || host_lower.ends_with(".local")
                || host_lower.ends_with(".internal")
                || host_lower.ends_with(".localdomain")
            {
                return Err(ClawzError::Validation(
                    "SSRF protection: internal hostnames are not allowed".into(),
                ));
            }
        } else {
            return Err(ClawzError::Validation("URL has no host".into()));
        }

        Ok(())
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL with configurable method, headers, and body. Returns status, headers, and body content. SSRF-protected."
    }

    fn primitive(&self) -> ActionPrimitive {
        ActionPrimitive::Read
    }
    fn risk(&self) -> RiskLevel {
        RiskLevel::Low
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_fetch".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch (http/https only)"
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"],
                        "description": "HTTP method (default: GET)"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional request headers as key-value pairs",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional request body (for POST/PUT/PATCH)"
                    },
                    "max_redirects": {
                        "type": "integer",
                        "description": "Maximum number of redirects to follow (default: 5)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Request timeout in seconds (default: 30)"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ClawzError> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("url required".into()))?;

        // SSRF protection
        Self::check_ssrf(url)?;

        let method = args["method"].as_str().unwrap_or("GET").to_uppercase();
        let timeout_secs = args["timeout_secs"]
            .as_u64()
            .unwrap_or(ctx.config.timeout_secs);
        let max_redirects = args["max_redirects"].as_u64().unwrap_or(5) as usize;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(max_redirects))
            .build()
            .map_err(|e| ClawzError::Tool(format!("failed to build HTTP client: {e}")))?;

        let req_method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| ClawzError::Validation(format!("unknown method: {method}")))?;

        let mut request = client.request(req_method, url);

        // Apply headers
        if let Some(headers_obj) = args["headers"].as_object() {
            for (k, v) in headers_obj {
                if let Some(val) = v.as_str() {
                    request = request.header(k.as_str(), val);
                }
            }
        }

        // Apply body
        if let Some(body_str) = args["body"].as_str() {
            request = request.body(body_str.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("request failed: {e}")))?;

        let status = response.status().as_u16();
        let resp_headers: serde_json::Value = {
            let mut map = serde_json::Map::new();
            for (k, v) in response.headers().iter() {
                if let Ok(vs) = v.to_str() {
                    map.insert(
                        k.as_str().to_string(),
                        serde_json::Value::String(vs.to_string()),
                    );
                }
            }
            serde_json::Value::Object(map)
        };

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to read response body: {e}")))?;

        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(ClawzError::Tool(format!(
                "response body exceeds {MAX_RESPONSE_BYTES} byte limit"
            )));
        }

        let body = if content_type.contains("text")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type.contains("javascript")
            || bytes.iter().all(|b| b.is_ascii() || *b >= 0x80)
        {
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            format!("base64:{}", BASE64.encode(&bytes))
        };

        let output = serde_json::json!({
            "status": status,
            "headers": resp_headers,
            "body": body,
            "size_bytes": bytes.len()
        })
        .to_string();

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error: status >= 400,
        })
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
    fn test_web_fetch_name() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
    }

    #[test]
    fn test_web_fetch_schema() {
        let tool = WebFetchTool::new();
        let schema = tool.schema();
        assert_eq!(schema.name, "web_fetch");
        assert!(schema.parameters["properties"]["url"].is_object());
        assert!(schema.parameters["properties"]["method"].is_object());
        assert!(schema.parameters["properties"]["headers"].is_object());
    }

    #[tokio::test]
    async fn test_web_fetch_execute_missing_url() {
        let tool = WebFetchTool::new();
        let ctx = make_ctx();
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssrf_private_ip_blocked() {
        let tool = WebFetchTool::new();
        let ctx = make_ctx();
        // 192.168.x.x private
        let result = tool
            .execute(&ctx, serde_json::json!({"url": "http://192.168.1.1/"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("SSRF"));
    }

    #[tokio::test]
    async fn test_ssrf_localhost_blocked() {
        let tool = WebFetchTool::new();
        let ctx = make_ctx();
        let result = tool
            .execute(&ctx, serde_json::json!({"url": "http://localhost/"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssrf_10_x_blocked() {
        let tool = WebFetchTool::new();
        let ctx = make_ctx();
        let result = tool
            .execute(&ctx, serde_json::json!({"url": "http://10.0.0.1/"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssrf_172_16_blocked() {
        let tool = WebFetchTool::new();
        let ctx = make_ctx();
        let result = tool
            .execute(&ctx, serde_json::json!({"url": "http://172.16.0.1/"}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_ssrf_file_scheme_blocked() {
        let result = WebFetchTool::check_ssrf("file:///etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_ssrf_public_ip_allowed() {
        // 8.8.8.8 is public
        let result = WebFetchTool::check_ssrf("https://8.8.8.8/");
        assert!(result.is_ok());
    }
}
