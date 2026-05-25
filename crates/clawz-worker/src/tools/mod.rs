//! Tool subsystem for agent execution of external operations.
//!
//! This module provides a registry of tools (implementations of the [`Tool`] trait),
//! including built-in tools for web access, file operations, Docker management,
//! browser automation, and MCP (Model Context Protocol) integrations.
//!
//! ## Sub-modules
//!
//! - [`tool_trait`]  — [`Tool`] trait and related types (config, context, execution)
//! - [`registry`]    — [`ToolRegistry`] for discovering and managing available tools
//! - [`builtin`]     — Built-in tool implementations (web_fetch, pdf_read, file_ops, shell, etc.)
//! - [`browser`]     — Browser automation tool (`BrowserManager`)
//! - [`docker`]      — Docker container management tool (`DockerToolManager`, `RunningTool`)
//! - [`mcp`]         — Model Context Protocol (MCP) integration (`McpServerManager`, `McpClient`)
//!
//! ## Cross-module dependencies
//!
//! // Dependency: `clawz_core::traits::Tool` defines the contract all tools must implement.
//! // Dependency: `clawz_core::types::tool::ToolResult` for execution results.
//! // Dependency: Runtime pipeline calls tools via the [`Tool::execute`] method.
//! // Dependency: Tools may call back to the agent runtime for sub-tasks (multi-turn).
//!
//! ## Example: Registering and Using a Tool
//!
//! ```ignore
//! let mut registry = ToolRegistry::new();
//! registry.register_builtin_tools().await?;
//!
//! let tool = registry.get("web_fetch")?;
//! let config = serde_json::json!({
//!     "url": "https://example.com",
//! });
//!
//! let result = tool.execute(&config, context).await?;
//! println!("{:?}", result);
//! ```

#[path = "trait.rs"]
pub mod tool_trait;
pub mod builtin;
pub mod browser;
pub mod docker;
pub mod mcp;
pub mod registry;

pub use tool_trait::{Tool, ToolConfig, ToolContext};
pub use registry::ToolRegistry;
pub use builtin::web_fetch::WebFetchTool;
pub use docker::{DockerToolEntry, DockerToolManager, RunningTool, ToolContainerConfig, ToolHealth, docker_tool_library};
pub use mcp::{McpClient, McpServerManager};
pub use browser::BrowserManager;
