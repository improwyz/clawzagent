use crate::tools::tool_trait::{Tool, ToolContext};
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::{ToolResult, ToolSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }

    /// Search using DuckDuckGo Instant Answer API (free, no key required).
    async fn search_duckduckgo(query: &str, max_results: usize) -> Result<Vec<SearchResult>, ClawzError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("ClawZ-Agent/1.0")
            .build()
            .map_err(|e| ClawzError::Tool(format!("HTTP client build failed: {e}")))?;

        // DuckDuckGo HTML search — we parse the HTML results
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );

        let html = client
            .get(&url)
            .header("Accept", "text/html")
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("DuckDuckGo request failed: {e}")))?
            .text()
            .await
            .map_err(|e| ClawzError::Tool(format!("DuckDuckGo response read failed: {e}")))?;

        // Simple HTML parser — extract result blocks
        let results = parse_ddg_html(&html, max_results);
        Ok(results)
    }

    /// Search using SearXNG (self-hosted) if configured.
    async fn search_searxng(
        base_url: &str,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ClawzError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| ClawzError::Tool(format!("HTTP client build failed: {e}")))?;

        let url = format!(
            "{}/search?q={}&format=json&categories=general",
            base_url.trim_end_matches('/'),
            urlencoding::encode(query)
        );

        let resp: Value = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClawzError::Tool(format!("SearXNG request failed: {e}")))?
            .json()
            .await
            .map_err(|e| ClawzError::Tool(format!("SearXNG response parse failed: {e}")))?;

        let results = resp["results"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .take(max_results)
            .map(|r| SearchResult {
                title: r["title"].as_str().unwrap_or("").to_string(),
                url: r["url"].as_str().unwrap_or("").to_string(),
                snippet: r["content"].as_str().unwrap_or("").to_string(),
            })
            .collect();

        Ok(results)
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse DuckDuckGo HTML search results.
fn parse_ddg_html(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut remaining = html;

    while results.len() < max_results {
        // Find result block
        let result_start = match remaining.find("class=\"result__body\"") {
            Some(pos) => pos,
            None => break,
        };
        remaining = &remaining[result_start..];

        // Extract title and URL
        let title = extract_between(remaining, "class=\"result__a\"", "</a>")
            .map(strip_html_tags)
            .unwrap_or_default();

        let url = extract_between(remaining, "class=\"result__url\"", "</a>")
            .map(|s| strip_html_tags(s).trim().to_string())
            .unwrap_or_default();

        // Clean up URL
        let clean_url = if url.starts_with("http") {
            url.clone()
        } else {
            format!("https://{}", url.trim_start_matches('/'))
        };

        let snippet = extract_between(remaining, "class=\"result__snippet\"", "</a>")
            .map(strip_html_tags)
            .unwrap_or_default();

        if !title.is_empty() && !clean_url.is_empty() {
            results.push(SearchResult {
                title,
                url: clean_url,
                snippet,
            });
        }

        // Advance past this result
        if let Some(end) = remaining[1..].find("class=\"result__body\"") {
            remaining = &remaining[end + 1..];
        } else {
            break;
        }
    }

    results
}

fn extract_between<'a>(haystack: &'a str, start_marker: &str, end_tag: &str) -> Option<&'a str> {
    let start = haystack.find(start_marker)?;
    let after_marker = &haystack[start..];
    let tag_end = after_marker.find('>')?;
    let content_start = tag_end + 1;
    let content = &after_marker[content_start..];
    let end = content.find(end_tag)?;
    Some(&content[..end])
}

fn strip_html_tags(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information. Returns a list of {title, url, snippet} results."
    }


    fn primitive(&self) -> ActionPrimitive { ActionPrimitive::Read }
    fn risk(&self) -> RiskLevel { RiskLevel::Low }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_search".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 10, max: 20)"
                    },
                    "engine": {
                        "type": "string",
                        "enum": ["duckduckgo", "searxng"],
                        "description": "Search engine to use (default: duckduckgo)"
                    },
                    "searxng_url": {
                        "type": "string",
                        "description": "SearXNG base URL (required when engine=searxng)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: Value,
    ) -> Result<ToolResult, ClawzError> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("query required".into()))?;

        let max_results = args["max_results"]
            .as_u64()
            .unwrap_or(10)
            .min(20) as usize;

        let engine = args["engine"].as_str().unwrap_or("duckduckgo");

        // Check environment variable for searxng URL override
        let searxng_base = args["searxng_url"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| std::env::var("SEARXNG_URL").ok());

        let results = match engine {
            "searxng" => {
                let base = searxng_base.ok_or_else(|| {
                    ClawzError::Validation("searxng_url required when engine=searxng".into())
                })?;
                Self::search_searxng(&base, query, max_results).await?
            }
            _ => {
                // Default to DuckDuckGo, fall back to SearXNG if configured
                if let Some(base) = searxng_base {
                    Self::search_searxng(&base, query, max_results).await
                        .unwrap_or_else(|_| vec![])
                } else {
                    Self::search_duckduckgo(query, max_results).await?
                }
            }
        };

        let output = serde_json::json!({
            "query": query,
            "engine": engine,
            "results": results,
            "total": results.len()
        })
        .to_string();

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error: false,
        })
    }
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
                | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{:02X}", b)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_search_name() {
        let tool = WebSearchTool::new();
        assert_eq!(tool.name(), "web_search");
    }

    #[test]
    fn test_web_search_schema() {
        let tool = WebSearchTool::new();
        let schema = tool.schema();
        assert_eq!(schema.name, "web_search");
        assert!(schema.parameters["properties"]["query"].is_object());
        assert!(schema.parameters["required"].as_array().unwrap().contains(&serde_json::json!("query")));
    }

    #[tokio::test]
    async fn test_web_search_missing_query() {
        let tool = WebSearchTool::new();
        let ctx = ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: crate::tools::ToolConfig::default(),
        };
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<b>Hello</b> &amp; <i>World</i>";
        let stripped = strip_html_tags(html);
        assert_eq!(stripped, "Hello & World");
    }

    #[test]
    fn test_url_encoding() {
        let encoded = urlencoding::encode("hello world & more");
        assert_eq!(encoded, "hello+world+%26+more");
    }

    #[test]
    fn test_parse_ddg_html_empty() {
        let results = parse_ddg_html("<html><body></body></html>", 10);
        assert!(results.is_empty());
    }
}
