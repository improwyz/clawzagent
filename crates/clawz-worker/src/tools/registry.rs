//! Tool registry and lifecycle management.
//!
//! This module implements [`ToolRegistry`], which maintains a map of available
//! tools (implementations of the [`Tool`] trait) and provides discovery, listing,
//! and execution capabilities.
//!
//! ## Features
//!
//! - **Registration**: Register individual tools or built-in tool suites
//! - **Discovery**: Look up tools by name, list all available tools
//! - **Categories**: Organize tools by category (builtin, custom, mcp, etc.)
//! - **Enablement**: Enable/disable tools at runtime
//! - **Schema export**: Generate OpenAPI-like schemas for each tool
//!
//! ## Cross-module dependencies
//!
//! // Dependency: `Tool` trait from [`super::tool_trait`] for the contract.
//! // Dependency: Individual tool implementations in [`super::builtin`], [`super::browser`], etc.
//! // Dependency: `clawz_core::types::ToolSchema` for exposing tool metadata.

use super::tool_trait::{Tool, ToolContext};
use clawz_core::error::ClawzError;
use clawz_core::types::{ToolRegistration, ToolResult, ToolSchema};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

struct ToolEntry {
    tool: Arc<dyn Tool>,
    enabled: bool,
    category: String,
}

pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, ToolEntry>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a built-in tool with "builtin" category.
    pub async fn register(&self, tool: Arc<dyn Tool>) {
        self.register_with_category(tool, "builtin").await;
    }

    /// Register a tool with an explicit category.
    pub async fn register_with_category(&self, tool: Arc<dyn Tool>, category: &str) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(
            name,
            ToolEntry {
                tool,
                enabled: true,
                category: category.to_string(),
            },
        );
    }

    pub async fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let guard = self.tools.read().await;
        let entry = guard.get(name)?;
        if entry.enabled {
            Some(entry.tool.clone())
        } else {
            None
        }
    }

    pub async fn list(&self) -> Vec<ToolSchema> {
        self.tools
            .read()
            .await
            .values()
            .filter(|e| e.enabled)
            .map(|e| e.tool.schema())
            .collect()
    }

    /// List all tools as ToolRegistration structs (includes id, category, version, enabled).
    pub async fn list_registrations(&self) -> Vec<ToolRegistration> {
        self.tools
            .read()
            .await
            .values()
            .map(|e| ToolRegistration::new(e.tool.schema(), e.category.clone()))
            .collect()
    }

    /// List tools filtered by category.
    pub async fn list_by_category(&self, category: &str) -> Vec<ToolSchema> {
        self.tools
            .read()
            .await
            .values()
            .filter(|e| e.enabled && e.category == category)
            .map(|e| e.tool.schema())
            .collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ClawzError> {
        let tool = self
            .get(name)
            .await
            .ok_or_else(|| ClawzError::Tool(format!("tool not found: {name}")))?;

        tool.execute(ctx, args).await
    }

    pub async fn names(&self) -> Vec<String> {
        self.tools.read().await.keys().cloned().collect()
    }

    /// Enable a tool by name. Returns false if tool not found.
    pub async fn enable(&self, name: &str) -> bool {
        let mut guard = self.tools.write().await;
        if let Some(entry) = guard.get_mut(name) {
            entry.enabled = true;
            true
        } else {
            false
        }
    }

    /// Disable a tool by name. Returns false if tool not found.
    pub async fn disable(&self, name: &str) -> bool {
        let mut guard = self.tools.write().await;
        if let Some(entry) = guard.get_mut(name) {
            entry.enabled = false;
            true
        } else {
            false
        }
    }

    /// Register all built-in tools.
    pub async fn register_builtins(&self) {
        crate::tools::builtin::register_builtins(self).await;
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tool_trait::{Tool, ToolConfig, ToolContext};
    use super::*;
    use async_trait::async_trait;
    use clawz_core::types::{ToolResult, ToolSchema};

    struct DummyTool {
        name: String,
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "dummy"
        }

        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: self.name.clone(),
                description: "dummy".into(),
                parameters: serde_json::json!({}),
            }
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args: serde_json::Value,
        ) -> Result<ToolResult, ClawzError> {
            Ok(ToolResult {
                tool_call_id: String::new(),
                output: "ok".into(),
                is_error: false,
            })
        }
    }

    #[tokio::test]
    async fn test_registry_register_and_get() {
        let reg = ToolRegistry::new();
        let tool = Arc::new(DummyTool {
            name: "dummy".into(),
        });
        reg.register(tool).await;

        let got = reg.get("dummy").await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().name(), "dummy");
    }

    #[tokio::test]
    async fn test_registry_list() {
        let reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool {
            name: "tool_a".into(),
        }))
        .await;
        reg.register(Arc::new(DummyTool {
            name: "tool_b".into(),
        }))
        .await;

        let list = reg.list().await;
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_registry_execute_found() {
        let reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool {
            name: "echo".into(),
        }))
        .await;

        let ctx = ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: ToolConfig::default(),
        };
        let result = reg
            .execute("echo", &ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.output, "ok");
    }

    #[tokio::test]
    async fn test_registry_execute_not_found() {
        let reg = ToolRegistry::new();
        let ctx = ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: ToolConfig::default(),
        };
        let result = reg.execute("missing", &ctx, serde_json::json!({})).await;
        assert!(result.is_err());
        let err_str = format!("{}", result.unwrap_err());
        assert!(err_str.contains("missing"));
    }

    #[tokio::test]
    async fn test_registry_names() {
        let reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool {
            name: "alpha".into(),
        }))
        .await;
        reg.register(Arc::new(DummyTool {
            name: "beta".into(),
        }))
        .await;

        let names = reg.names().await;
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha".into()));
        assert!(names.contains(&"beta".into()));
    }
}
