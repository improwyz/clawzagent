use crate::tools::tool_trait::{Tool, ToolContext};
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::{ToolResult, ToolSchema};
use serde_json::Value;
const MAX_PDF_BYTES: usize = 50 * 1024 * 1024; // 50 MB

pub struct PdfReadTool;

impl PdfReadTool {
    pub fn new() -> Self {
        Self
    }

    /// Attempt basic PDF text extraction by parsing raw bytes.
    /// This is a simplified extractor — it reads text stream contents directly.
    fn extract_text_from_bytes(pdf_bytes: &[u8]) -> Vec<String> {
        let content = String::from_utf8_lossy(pdf_bytes);
        let mut pages: Vec<String> = Vec::new();
        let mut current_page_text = String::new();

        // Very simple stream content extraction
        // A proper implementation would use a PDF parser library (lopdf, pdf-extract, etc.)
        // This extracts readable text from PDF streams heuristically
        let mut i = 0;
        let bytes = pdf_bytes;

        while i < bytes.len() {
            // Look for text stream markers
            if i + 6 < bytes.len() && &bytes[i..i + 7] == b"stream\n" {
                // Find endstream
                let start = i + 7;
                if let Some(end_pos) = content[start..].find("endstream") {
                    let stream = &content[start..start + end_pos];
                    // Extract text between Td/Tj operators
                    current_page_text.push_str(&extract_pdf_text_ops(stream));
                    i = start + end_pos;
                }
            }

            // Look for page markers
            if i + 5 < bytes.len() && &bytes[i..i + 6] == b"/Page "
                && !current_page_text.trim().is_empty() {
                    pages.push(current_page_text.trim().to_string());
                    current_page_text = String::new();
                }

            i += 1;
        }

        if !current_page_text.trim().is_empty() {
            pages.push(current_page_text.trim().to_string());
        }

        // Fallback: try to extract any printable text from the whole document
        if pages.is_empty() {
            let text = extract_printable_text(&content);
            if !text.is_empty() {
                pages.push(text);
            }
        }

        pages
    }
}

/// Extract text from PDF content stream operators (Tj, TJ, ').
fn extract_pdf_text_ops(stream: &str) -> String {
    let mut result = String::new();
    let mut chars = stream.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '(' {
            // PDF string literal
            let mut depth = 1;
            let mut s = String::new();
            for inner in chars.by_ref() {
                match inner {
                    '(' => { depth += 1; s.push('('); }
                    ')' => {
                        depth -= 1;
                        if depth == 0 { break; }
                        s.push(')');
                    }
                    '\\' => {
                        // Skip escape sequences
                    }
                    _ => s.push(inner),
                }
            }
            // Look ahead for Tj or ' operator
            let rest: String = chars.clone().take(10).collect();
            let rest_trimmed = rest.trim_start();
            if rest_trimmed.starts_with("Tj") || rest_trimmed.starts_with("'") {
                let printable: String = s.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').collect();
                if !printable.is_empty() {
                    result.push_str(&printable);
                    result.push(' ');
                }
            }
        }
    }

    result
}

/// Extract printable ASCII text sequences from raw PDF bytes.
fn extract_printable_text(content: &str) -> String {
    let mut result = String::new();
    let mut run = String::new();

    for c in content.chars() {
        if c.is_ascii_graphic() || c == ' ' || c == '\n' || c == '\r' {
            run.push(c);
        } else {
            if run.len() >= 4 {
                result.push_str(&run);
                result.push('\n');
            }
            run.clear();
        }
    }

    if run.len() >= 4 {
        result.push_str(&run);
    }

    result
}

impl Default for PdfReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PdfReadTool {
    fn name(&self) -> &str {
        "pdf_read"
    }

    fn description(&self) -> &str {
        "Read a PDF file from a path or URL and extract its text content page by page."
    }


    fn primitive(&self) -> ActionPrimitive { ActionPrimitive::Read }
    fn risk(&self) -> RiskLevel { RiskLevel::Low }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "pdf_read".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Path to a local PDF file or HTTP/HTTPS URL"
                    },
                    "pages": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "description": "Specific page numbers to extract (1-indexed). If omitted, all pages are extracted."
                    },
                    "max_pages": {
                        "type": "integer",
                        "description": "Maximum number of pages to extract (default: 50)"
                    }
                },
                "required": ["source"]
            }),
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: Value,
    ) -> Result<ToolResult, ClawzError> {
        let source = args["source"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("source required".into()))?;

        let max_pages = args["max_pages"].as_u64().unwrap_or(50) as usize;

        let page_filter: Option<Vec<usize>> = args["pages"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect());

        // Load PDF bytes
        let pdf_bytes = if source.starts_with("http://") || source.starts_with("https://") {
            // Fetch from URL
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| ClawzError::Tool(format!("HTTP client failed: {e}")))?;

            let resp = client
                .get(source)
                .send()
                .await
                .map_err(|e| ClawzError::Tool(format!("failed to fetch PDF: {e}")))?;

            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ClawzError::Tool(format!("failed to read PDF response: {e}")))?;

            if bytes.len() > MAX_PDF_BYTES {
                return Err(ClawzError::Tool(format!(
                    "PDF too large: {} bytes (max {})",
                    bytes.len(),
                    MAX_PDF_BYTES
                )));
            }

            bytes.to_vec()
        } else {
            // Read local file
            let metadata = std::fs::metadata(source)
                .map_err(|e| ClawzError::Tool(format!("cannot stat '{}': {e}", source)))?;

            if metadata.len() as usize > MAX_PDF_BYTES {
                return Err(ClawzError::Tool(format!(
                    "PDF file too large: {} bytes",
                    metadata.len()
                )));
            }

            std::fs::read(source)
                .map_err(|e| ClawzError::Tool(format!("failed to read file '{}': {e}", source)))?
        };

        // Validate PDF header
        if pdf_bytes.len() < 5 || &pdf_bytes[..5] != b"%PDF-" {
            return Err(ClawzError::Validation(format!(
                "'{}' does not appear to be a valid PDF file",
                source
            )));
        }

        // Extract text
        let all_pages = Self::extract_text_from_bytes(&pdf_bytes);

        // Apply page filter
        let selected_pages: Vec<(usize, &str)> = all_pages
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                if let Some(ref filter) = page_filter {
                    filter.contains(&(*i + 1)) // 1-indexed
                } else {
                    *i < max_pages
                }
            })
            .map(|(i, text)| (i + 1, text.as_str()))
            .collect();

        let pages_json: Vec<Value> = selected_pages
            .iter()
            .map(|(page_num, text)| {
                serde_json::json!({
                    "page": page_num,
                    "text": text,
                    "char_count": text.len()
                })
            })
            .collect();

        let total_chars: usize = pages_json
            .iter()
            .map(|p| p["char_count"].as_u64().unwrap_or(0) as usize)
            .sum();

        let output = serde_json::json!({
            "source": source,
            "total_pages_found": all_pages.len(),
            "pages_extracted": pages_json.len(),
            "total_chars": total_chars,
            "pages": pages_json
        })
        .to_string();

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error: false,
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
    fn test_pdf_read_name() {
        assert_eq!(PdfReadTool::new().name(), "pdf_read");
    }

    #[test]
    fn test_pdf_read_schema() {
        let schema = PdfReadTool::new().schema();
        assert_eq!(schema.name, "pdf_read");
        assert!(schema.parameters["properties"]["source"].is_object());
    }

    #[tokio::test]
    async fn test_missing_source() {
        let tool = PdfReadTool::new();
        let ctx = make_ctx();
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_pdf_header() {
        let tool = PdfReadTool::new();
        let ctx = make_ctx();

        // Write a fake "PDF" file
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"not a pdf at all").unwrap();

        let result = tool
            .execute(
                &ctx,
                serde_json::json!({"source": tmp.path().to_str().unwrap()}),
            )
            .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("PDF") || msg.contains("valid"), "msg: {}", msg);
    }

    #[test]
    fn test_extract_text_empty_pdf() {
        // Minimal PDF structure — just check it doesn't panic
        let fake_pdf = b"%PDF-1.4\n%%EOF";
        let pages = PdfReadTool::extract_text_from_bytes(fake_pdf);
        // May or may not extract anything, but shouldn't panic
        let _ = pages;
    }
}
