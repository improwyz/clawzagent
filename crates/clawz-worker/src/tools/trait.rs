//! Tool trait and execution context definitions.
//!
//! This module defines the [`Tool`] trait contract that all tools must implement,
//! as well as the [`ToolContext`] and [`ToolConfig`] structures that provide
//! runtime configuration and contextual information during tool execution.
//!
//! ## The Tool Trait
//!
//! Every tool must implement:
//! - `name()` — Unique identifier within the registry
//! - `description()` — Human-readable description for documentation/UI
//! - `schema()` — OpenAPI-like parameter schema
//! - `execute()` — Async method that runs the tool logic
//!
//! ## Execution Context
//!
//! [`ToolContext`] carries:
//! - `agent_id` — The agent invoking this tool
//! - `conversation_id` — The conversation this tool call belongs to
//! - `user_id` — Optional end-user identifier
//! - `config` — [`ToolConfig`] with timeouts and retry settings
//!
//! ## Cross-module dependencies
//!
//! // Dependency: `clawz_core::types::ToolSchema` for tool metadata schemas.
//! // Dependency: `clawz_core::types::ToolResult` for execution results.
//! // Dependency: `Tool` is implemented by all modules in [`super`].

use std::sync::Arc;

use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::{ActionPrimitive, RiskLevel, ToolResult, ToolSchema};
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ClawzError>;

    /// The fundamental action primitive this tool performs.
    fn primitive(&self) -> ActionPrimitive {
        ActionPrimitive::Execute
    }

    /// The risk level of this tool's actions.
    fn risk(&self) -> RiskLevel {
        RiskLevel::Medium
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub agent_id: String,
    pub conversation_id: String,
    pub user_id: Option<String>,
    pub config: ToolConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            max_retries: 3,
        }
    }
}

/// Bridge a worker-local tool into [`clawz_core::traits::Tool`] for the runtime pipeline.
pub fn bridge_to_core(tool: Arc<dyn Tool>) -> Arc<dyn clawz_core::traits::Tool> {
    Arc::new(CoreToolBridge(tool))
}

struct CoreToolBridge(Arc<dyn Tool>);

#[async_trait]
impl clawz_core::traits::Tool for CoreToolBridge {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn schema(&self) -> ToolSchema {
        self.0.schema()
    }

    async fn execute(
        &self,
        ctx: &clawz_core::traits::ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ClawzError> {
        let worker_ctx = ToolContext {
            agent_id: ctx.agent_id.clone(),
            conversation_id: ctx.conversation_id.clone(),
            user_id: None,
            config: ToolConfig::default(),
        };
        self.0.execute(&worker_ctx, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            "mock_tool"
        }

        fn description(&self) -> &str {
            "A mock tool for testing"
        }

        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "mock_tool".into(),
                description: "A mock tool for testing".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    },
                    "required": ["input"]
                }),
            }
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            args: serde_json::Value,
        ) -> Result<ToolResult, ClawzError> {
            let input = args["input"].as_str().unwrap_or("");
            Ok(ToolResult {
                tool_call_id: String::new(),
                output: format!("processed: {}", input),
                is_error: false,
            })
        }
    }

    #[test]
    fn test_tool_name() {
        let tool = MockTool;
        assert_eq!(tool.name(), "mock_tool");
    }

    #[test]
    fn test_tool_description() {
        let tool = MockTool;
        assert_eq!(tool.description(), "A mock tool for testing");
    }

    #[test]
    fn test_tool_schema() {
        let tool = MockTool;
        let schema = tool.schema();
        assert_eq!(schema.name, "mock_tool");
        assert_eq!(schema.description, "A mock tool for testing");
        assert!(schema.parameters["properties"]["input"].is_object());
    }

    #[tokio::test]
    async fn test_tool_execute() {
        let tool = MockTool;
        let ctx = ToolContext {
            agent_id: "agent-1".into(),
            conversation_id: "conv-1".into(),
            user_id: None,
            config: ToolConfig::default(),
        };
        let result = tool
            .execute(&ctx, serde_json::json!({"input": "hello"}))
            .await
            .unwrap();
        assert_eq!(result.output, "processed: hello");
        assert!(!result.is_error);
    }

    #[test]
    fn test_tool_config_default() {
        let config = ToolConfig::default();
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_tool_context_serde() {
        let ctx = ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: Some("u".into()),
            config: ToolConfig::default(),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let de: ToolContext = serde_json::from_str(&json).unwrap();
        assert_eq!(de.agent_id, "a");
        assert_eq!(de.conversation_id, "c");
        assert_eq!(de.user_id, Some("u".into()));
    }

    #[test]
    fn test_default_primitive_is_execute() {
        let tool = MockTool;
        assert_eq!(tool.primitive(), ActionPrimitive::Execute);
    }

    #[test]
    fn test_default_risk_is_medium() {
        let tool = MockTool;
        assert_eq!(tool.risk(), RiskLevel::Medium);
    }
}
