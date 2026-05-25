//! Built-in tool implementations for common agent operations.
//!
//! This module provides the standard tools that are available in all agent
//! runtimes, including web access, file operations, code execution, and more.
//!
//! ## Available Tools
//!
//! - [`web_fetch`]     — HTTP GET/POST requests with automatic parsing
//! - [`web_search`]    — Search the web via an external search API
//! - [`browser`]       — Full browser automation with Chrome DevTools Protocol
//! - [`file_ops`]      — Read, write, delete, and list files on the local filesystem
//! - [`shell`]         — Execute shell commands (with security restrictions)
//! - [`git`]           — Git operations (clone, push, pull, commit, etc.)
//! - [`calculator`]    — Mathematical expression evaluation
//! - [`image_gen`]     — Generate images via DALL-E or similar APIs
//! - [`pdf_read`]      — Extract text and metadata from PDF files
//! - [`knowledge_base`] — Semantic search over uploaded documents
//! - [`escalate`]      — Request human intervention or approval
//!
//! ## Cross-module dependencies
//!
//! // Dependency: All tools implement the [`Tool`] trait from [`super::tool_trait`].
//! // Dependency: Tools are registered via [`register_builtins`].

pub mod browser;
pub mod calculator;
pub mod escalate;
pub mod file_ops;
pub mod git;
pub mod image_gen;
pub mod knowledge_base;
pub mod pdf_read;
pub mod shell;
pub mod web_fetch;
pub mod web_search;

use crate::tools::{ToolRegistry, tool_trait::Tool};
use std::sync::Arc;

/// Register all built-in tools into a registry.
pub async fn register_builtins(registry: &ToolRegistry) {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(web_fetch::WebFetchTool::new()),
        Arc::new(browser::BrowserTool::new()),
        Arc::new(web_search::WebSearchTool::new()),
        Arc::new(file_ops::FileOpsTool::new()),
        Arc::new(shell::ShellTool::new()),
        Arc::new(git::GitTool::new()),
        Arc::new(calculator::CalculatorTool::new()),
        Arc::new(image_gen::ImageGenTool::new()),
        Arc::new(pdf_read::PdfReadTool::new()),
        Arc::new(knowledge_base::KnowledgeBaseTool::new()),
        Arc::new(escalate::EscalateTool::new()),
    ];
    for tool in tools {
        registry.register(tool).await;
    }
}
