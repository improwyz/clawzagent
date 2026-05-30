//! SelectProviderStep — builds a ChatRequest and calls the ProviderRouter.
//!
//! Responsibilities:
//! - Reads agent config (model, temperature, max_tokens) from context metadata.
//! - Assembles the full message list with system prompt (produced by
//!   [`RetrieveContextStep`](crate::runtime::steps::context::RetrieveContextStep)).
//! - Calls `ProviderRouter::route()`.
//! - Stores the ChatResponse in context metadata.
//! - Records cost via `CostTracker` and feeds it into `ctx.cost_accumulated`.
//!
//! # Cross-module dependencies
//! - Reads `META_SYSTEM_PROMPT` from the context step.
//! - Writes `META_CHAT_RESPONSE` and `META_MODEL_USED` for downstream steps
//!   (governance, streaming, persistence).
//! - Uses `crate::providers::router::ProviderRouter` for actual LLM calls.
//! - Uses `crate::providers::cost::CostTracker` for spend tracking.

use std::sync::Arc;

use async_trait::async_trait;
// Dependency: core error types, pipeline abstractions, and message types.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{PipelineContext, PipelineStep, StepOutcome},
    types::message::{ChatRequest, Message},
};

// Dependency: provider router and cost tracker from the worker crate.
use crate::providers::{cost::CostTracker, router::ProviderRouter};
// Dependency: metadata key produced by the context retrieval step.
use crate::runtime::steps::context::META_SYSTEM_PROMPT;

/// Metadata key under which the ChatResponse is stored.
///
/// Downstream steps (governance, streaming) read this to inspect or
/// forward the model's output.
pub const META_CHAT_RESPONSE: &str = "chat_response";
/// Metadata key for the model that was used.
pub const META_MODEL_USED: &str = "model_used";

/// Pipeline step that selects an LLM provider, sends the chat request,
/// and records the response.
///
/// This is the most expensive step in the pipeline (network I/O + GPU
/// inference), so it is placed after context retrieval and before tool
/// execution so that tool schemas are available in the request.
pub struct SelectProviderStep {
    /// Shared provider router; selects the concrete backend based on model name.
    router: Arc<ProviderRouter>,
    /// Tracks per-request and cumulative cost.
    ///
    /// Dependency: feeds into `CostRepo` in `clawz_core::db`.
    cost_tracker: Arc<CostTracker>,
    /// Target model identifier (e.g. "gpt-4", "claude-3-opus").
    model: String,
    /// Optional sampling temperature.  `None` lets the provider use its default.
    temperature: Option<f32>,
    /// Optional max-token limit.  `None` lets the provider use its default.
    max_tokens: Option<u32>,
}

impl SelectProviderStep {
    /// Create a new provider step with the given router, cost tracker, and model.
    pub fn new(
        router: Arc<ProviderRouter>,
        cost_tracker: Arc<CostTracker>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            router,
            cost_tracker,
            model: model.into(),
            temperature: None,
            max_tokens: None,
        }
    }

    /// Override the sampling temperature.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Override the max-token limit.
    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }
}

#[async_trait]
impl PipelineStep for SelectProviderStep {
    fn name(&self) -> &str {
        "select_provider"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome> {
        // Build message list: prepend system prompt if available.
        // The system prompt is constructed by RetrieveContextStep and may
        // contain RAG context snippets; we inject it as the first message
        // so the model sees it before any user/assistant history.
        let mut messages = Vec::new();
        if let Some(sp) = ctx.get_meta(META_SYSTEM_PROMPT) {
            if let Some(text) = sp.as_str() {
                if !text.is_empty() {
                    messages.push(Message::system(text));
                }
            }
        }
        let mut turn_messages = ctx.messages.clone();
        if clawz_core::deployment::DeploymentMode::from_env()
            == clawz_core::deployment::DeploymentMode::Standalone
        {
            crate::memory::compress::compress_messages(&mut turn_messages);
        }
        messages.extend(turn_messages);

        // Determine tools from metadata (tool schemas stored as JSON array).
        // Tool schemas are typically injected earlier in the pipeline by a
        // tool-registry step or by AgentRuntime during construction.
        let tools: Vec<clawz_core::types::tool::ToolSchema> = ctx
            .get_meta("tool_schemas")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut request = ChatRequest::new(&self.model, messages);
        request.temperature = self.temperature;
        request.max_tokens = self.max_tokens;
        request.tools = tools;

        log::debug!(
            "[select_provider] routing request to model '{}' ({} messages)",
            self.model,
            request.messages.len()
        );

        let response = self
            .router
            .route(request)
            .await
            .map_err(|e| ClawzError::Provider(format!("provider routing failed: {e}")))?;

        // Track cost.
        // CostTracker uses per-model price tables; the result feeds into
        // both ctx.cost_accumulated (for budget guards) and the database
        // via CostRepo for billing/observability.
        let cost = self.cost_tracker.calculate_cost(
            &response.model,
            response.usage.prompt_tokens as u64,
            response.usage.completion_tokens as u64,
        );
        self.cost_tracker.record_cost(&response.model, cost).await;
        ctx.add_cost(cost);

        // Store response and model in metadata so downstream steps
        // (governance, streaming) can inspect them without re-invoking the provider.
        ctx.insert_meta(
            META_MODEL_USED,
            serde_json::Value::String(response.model.clone()),
        );
        ctx.insert_meta(
            META_CHAT_RESPONSE,
            serde_json::to_value(&response)
                .map_err(|e| ClawzError::Serialization(e.to_string()))?,
        );

        // Append assistant response to context message list.
        // This makes the model's output visible to tool execution and governance.
        if let Some(choice) = response.choices.first() {
            ctx.messages.push(choice.message.clone());
        }

        Ok(StepOutcome::Continue)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // A full integration test requires a live provider.  We just verify that
    // the step compiles and has the expected name.
    #[tokio::test]
    async fn test_step_name() {
        let router = Arc::new(
            ProviderRouter::new(crate::providers::ProviderRouterConfig::default())
                .await
                .unwrap(),
        );
        let tracker = Arc::new(CostTracker::new());
        let step = SelectProviderStep::new(router, tracker, "gpt-4");
        assert_eq!(step.name(), "select_provider");
    }
}
