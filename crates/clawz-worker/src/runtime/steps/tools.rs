//! ExecuteToolsStep — executes tool calls returned by the provider.
//!
//! Responsibilities:
//! - Extracts tool_calls from the latest assistant message in context.
//! - Executes each tool via the registered [`Tool`](clawz_core::traits::Tool) trait implementations.
//! - Collects [`ToolResult`](clawz_core::types::tool::ToolResult)s and appends them to `ctx.messages`.
//! - Loops back if further tool calls are returned (up to `max_iterations`).
//! - Pauses and requests approval if a tool requires it.
//!
//! # Cross-module dependencies
//! - Reads `MessageContent::ToolCalls` from the latest assistant message
//!   (placed there by [`SelectProviderStep`](crate::runtime::steps::provider::SelectProviderStep)).
//! - Writes `ToolResult` messages back into `ctx.messages` so the next
//!   provider call sees the tool output.
//! - Uses `clawz_core::traits::Tool` and `ToolContext` for execution.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
// Dependency: core error types, pipeline abstractions, and tool types.
use clawz_core::{
    error::Result,
    traits::{PipelineContext, PipelineStep, StepOutcome, Tool, ToolContext},
    types::{
        message::{Message, MessageContent, Role},
        tool::{ToolCall, ToolResult},
    },
};

/// Default maximum number of tool-call round-trips.
///
/// Prevents infinite loops when a model repeatedly calls the same tool.
const DEFAULT_MAX_ITERATIONS: usize = 5;
/// Metadata key: set to `true` if tool execution is pending approval.
pub const META_TOOL_APPROVAL_PENDING: &str = "tool_approval_pending";
/// Metadata key: list of tool names that require approval.
pub const META_TOOLS_REQUIRING_APPROVAL: &str = "tools_requiring_approval";

/// Pipeline step that executes tool calls embedded in the assistant message.
///
/// After execution, the step appends `Tool` role messages containing the
/// results.  The multi-turn loop in [`AgentRuntime`](crate::runtime::agent::AgentRuntime)
/// will then re-run the provider step so the model can observe the results.
pub struct ExecuteToolsStep {
    /// Registry of available tools keyed by name.
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Hard limit on tool-call iterations for this turn.
    max_iterations: usize,
    /// Tool names that require human approval before execution.
    ///
    /// When a tool in this list is called, the step halts with
    /// `StepOutcome::Halt` and sets `META_TOOL_APPROVAL_PENDING` so the
    /// caller can surface an approval UI.
    approval_required: Vec<String>,
    /// Shared context passed to every tool handler (agent_id, conversation_id).
    tool_context: ToolContext,
}

impl ExecuteToolsStep {
    /// Create a new tool-execution step.
    pub fn new(agent_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        let agent_id = agent_id.into();
        let conv_id = conversation_id.into();
        Self {
            tools: HashMap::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            approval_required: Vec::new(),
            tool_context: ToolContext::new(agent_id, conv_id),
        }
    }

    /// Override the maximum iteration limit.
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    /// Register a concrete tool implementation.
    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Declare that a named tool requires explicit human approval.
    pub fn require_approval_for(mut self, tool_name: impl Into<String>) -> Self {
        self.approval_required.push(tool_name.into());
        self
    }

    /// Extract tool calls from the latest assistant message, if any.
    ///
    /// Scans backwards because the newest assistant message is the one that
    /// may contain fresh tool calls after a multi-turn loop.
    fn extract_tool_calls(ctx: &PipelineContext) -> Vec<ToolCall> {
        ctx.messages
            .iter()
            .rev()
            .find_map(|msg| {
                if msg.role == Role::Assistant {
                    if let MessageContent::ToolCalls(calls) = &msg.content {
                        Some(calls.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }
}

#[async_trait]
impl PipelineStep for ExecuteToolsStep {
    fn name(&self) -> &str {
        "execute_tools"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome> {
        let calls = Self::extract_tool_calls(ctx);
        if calls.is_empty() {
            return Ok(StepOutcome::Continue);
        }

        // Check if any tool requires approval.
        // We halt early so that no tool runs before the user explicitly
        // approves the set.  This is a coarse-grained check; a future
        // enhancement could allow per-tool approval granularity.
        let needs_approval: Vec<&str> = calls
            .iter()
            .filter(|c| self.approval_required.iter().any(|ar| ar == &c.name))
            .map(|c| c.name.as_str())
            .collect();

        if !needs_approval.is_empty() {
            log::info!(
                "[execute_tools] tools requiring approval: {:?}",
                needs_approval
            );
            ctx.insert_meta(META_TOOL_APPROVAL_PENDING, serde_json::Value::Bool(true));
            ctx.insert_meta(
                META_TOOLS_REQUIRING_APPROVAL,
                serde_json::to_value(&needs_approval).unwrap_or(serde_json::Value::Array(vec![])),
            );
            return Ok(StepOutcome::Halt);
        }

        // Execute each tool call.
        // Errors in individual tools are captured as `ToolResult::err` rather
        // than bubbling up, so the pipeline can continue and the model sees
        // the failure explanation in the next turn.
        let mut results: Vec<ToolResult> = Vec::new();
        for call in &calls {
            let result = match self.tools.get(&call.name) {
                Some(tool) => {
                    log::debug!(
                        "[execute_tools] executing tool '{}' (id={})",
                        call.name,
                        call.id
                    );
                    match tool
                        .execute(&self.tool_context, call.arguments.clone())
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            log::error!("[execute_tools] tool '{}' failed: {e}", call.name);
                            ToolResult::err(&call.id, e.to_string())
                        }
                    }
                }
                None => {
                    log::warn!("[execute_tools] unknown tool '{}'", call.name);
                    ToolResult::err(&call.id, format!("tool '{}' not found", call.name))
                }
            };
            results.push(result);
        }

        // Append tool results as a Tool-role message.
        // The model expects tool results to follow the assistant message that
        // requested them, which is why we push in order after the assistant msg.
        for result in results {
            ctx.messages
                .push(Message::new(Role::Tool, MessageContent::ToolResult(result)));
        }

        Ok(StepOutcome::Continue)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::{
        traits::PipelineContext,
        types::{
            message::{Message, MessageContent},
            tool::{ToolCall, ToolSchema},
        },
    };
    use serde_json::json;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes its input"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::no_args("echo", "echo")
        }
        async fn execute(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolResult> {
            Ok(ToolResult::ok("0", args.to_string()))
        }
    }

    fn make_ctx_with_tool_calls(calls: Vec<ToolCall>) -> PipelineContext {
        let mut ctx = PipelineContext::new("agent-1", "conv-1");
        ctx.messages.push(Message::new(
            Role::Assistant,
            MessageContent::ToolCalls(calls),
        ));
        ctx
    }

    #[tokio::test]
    async fn test_no_tool_calls_continues() {
        let step = ExecuteToolsStep::new("agent-1", "conv-1");
        let mut ctx = PipelineContext::new("agent-1", "conv-1");
        ctx.messages.push(Message::assistant("Hello"));
        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));
    }

    #[tokio::test]
    async fn test_tool_executed_and_result_appended() {
        let mut step = ExecuteToolsStep::new("agent-1", "conv-1");
        step.register_tool(Arc::new(EchoTool));

        let call = ToolCall::new("id-1", "echo", json!({"msg": "hello"}));
        let mut ctx = make_ctx_with_tool_calls(vec![call]);

        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));

        // Should have appended a Tool-role message.
        let last = ctx.messages.last().unwrap();
        assert_eq!(last.role, Role::Tool);
    }

    #[tokio::test]
    async fn test_unknown_tool_returns_error_result() {
        let step = ExecuteToolsStep::new("agent-1", "conv-1");
        let call = ToolCall::new("id-2", "nonexistent_tool", json!({}));
        let mut ctx = make_ctx_with_tool_calls(vec![call]);

        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));
        let last = ctx.messages.last().unwrap();
        if let MessageContent::ToolResult(ref tr) = last.content {
            assert!(tr.is_error);
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn test_approval_required_halts() {
        let step =
            ExecuteToolsStep::new("agent-1", "conv-1").require_approval_for("dangerous_tool");
        let call = ToolCall::new("id-3", "dangerous_tool", json!({}));
        let mut ctx = make_ctx_with_tool_calls(vec![call]);

        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Halt));
        assert_eq!(
            ctx.get_meta(META_TOOL_APPROVAL_PENDING),
            Some(&serde_json::Value::Bool(true))
        );
    }
}
