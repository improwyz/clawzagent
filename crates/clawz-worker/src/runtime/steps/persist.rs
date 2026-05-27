//! PersistStateStep — saves the conversation turn to memory and updates agent state.
//!
//! Responsibilities:
//! - Saves all new messages to the [`MemoryBackend`](clawz_core::traits::MemoryBackend)
//!   (conversation history).
//! - Persists the updated [`AgentState`](clawz_core::types::agent::AgentState).
//! - Records accumulated cost (the cost is already added to `ctx.cost_accumulated`
//!   by [`SelectProviderStep`](crate::runtime::steps::provider::SelectProviderStep);
//!   this step merely persists the state).
//!
//! # Rollback behaviour
//! The memory backend is treated as append-only, so true deletion on rollback
//! is not currently implemented.  The `rollback` implementation logs a warning
//! and returns `Ok(())` so the pipeline can continue unwinding other steps.
//!
//! # Cross-module dependencies
//! - Reads `ctx.messages` and `ctx.agent_state`.
//! - Calls `MemoryBackend::save_message` and `MemoryBackend::save_agent_state`.
//! - Writes `META_PERSISTED_COUNT` into metadata for observability.

use std::sync::Arc;

use async_trait::async_trait;
// Dependency: core error types, memory trait, and pipeline abstractions.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{MemoryBackend, PipelineContext, PipelineStep, StepOutcome},
    types::{
        agent::AgentStatus,
        message::Role,
    },
};

/// Metadata key: number of messages persisted this turn.
pub const META_PERSISTED_COUNT: &str = "persisted_message_count";

/// Pipeline step that persists the current conversation turn to the memory backend.
///
/// Runs late in the pipeline (after governance) so that only approved
/// messages are saved to durable storage.
pub struct PersistStateStep {
    /// Memory backend for conversation history and agent-state storage.
    memory: Arc<dyn MemoryBackend>,
}

impl PersistStateStep {
    /// Create a new persistence step.
    pub fn new(memory: Arc<dyn MemoryBackend>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl PipelineStep for PersistStateStep {
    fn name(&self) -> &str {
        "persist_state"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome> {
        let mut saved = 0usize;

        // Save every non-system message in the current context.
        // We skip system messages because they are reconstructed on every
        // turn by RetrieveContextStep; persisting them would bloat history
        // with redundant copies.
        for msg in ctx.messages.iter().filter(|m| m.role != Role::System) {
            self.memory
                .save_message(&ctx.conversation_id, msg)
                .await
                .map_err(|e| ClawzError::Memory(format!("save_message failed: {e}")))?;
            saved += 1;
        }

        // Update and persist agent state.
        // We clone the current state, bump its message counter, and transition
        // back to Idle so the next turn starts from a clean slate.
        let mut state = ctx.agent_state.clone();
        state.mark_message();
        state.set_status(AgentStatus::Idle);

        self.memory
            .save_agent_state(&ctx.agent_id, &state)
            .await
            .map_err(|e| ClawzError::Memory(format!("save_agent_state failed: {e}")))?;

        ctx.agent_state = state;

        ctx.insert_meta(
            META_PERSISTED_COUNT,
            serde_json::Value::Number(saved.into()),
        );

        log::debug!(
            "[persist_state] saved {} messages for conversation '{}'",
            saved,
            ctx.conversation_id
        );

        Ok(StepOutcome::Continue)
    }

    async fn rollback(&self, ctx: &mut PipelineContext) -> Result<()> {
        // In a production system we would delete the messages we just saved.
        // For now, log and move on — the memory backend is append-only.
        // A future implementation could track saved message IDs and delete
        // them via a `MemoryBackend::delete_message` API.
        log::warn!(
            "[persist_state] rollback requested for conv '{}' (messages not deleted)",
            ctx.conversation_id
        );
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::{
        error::Result,
        traits::{MemoryBackend, MemoryEntry, PipelineContext},
        types::message::Message,
    };
    use std::sync::{Arc, Mutex};

    /// Test double that records every `save_message` call.
    struct RecordingMemory {
        saved: Mutex<Vec<String>>,
    }
    impl RecordingMemory {
        fn new() -> Self {
            Self {
                saved: Mutex::new(Vec::new()),
            }
        }
        fn saved_conversations(&self) -> Vec<String> {
            self.saved.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl MemoryBackend for RecordingMemory {
        async fn store(
            &self,
            _: &str,
            _: &str,
            _: serde_json::Value,
            _: Option<Vec<f32>>,
        ) -> Result<()> {
            Ok(())
        }
        async fn retrieve(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<serde_json::Value>> {
            Ok(None)
        }
        async fn search(
            &self,
            _: &str,
            _: Vec<f32>,
            _: usize,
        ) -> Result<Vec<MemoryEntry>> {
            Ok(vec![])
        }
        async fn get_conversation_history(
            &self,
            _: &str,
            _: usize,
        ) -> Result<Vec<Message>> {
            Ok(vec![])
        }
        async fn save_message(
            &self,
            conversation_id: &str,
            _msg: &Message,
        ) -> Result<()> {
            self.saved.lock().unwrap().push(conversation_id.to_string());
            Ok(())
        }
        async fn delete(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_messages_saved() {
        let mem = Arc::new(RecordingMemory::new());
        let step = PersistStateStep::new(mem.clone());

        let mut ctx = PipelineContext::new("agent-1", "conv-42");
        ctx.messages.push(Message::user("hi"));
        ctx.messages.push(Message::assistant("hello"));

        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));

        let saved = mem.saved_conversations();
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().all(|s| s == "conv-42"));
    }

    #[tokio::test]
    async fn test_system_messages_excluded() {
        let mem = Arc::new(RecordingMemory::new());
        let step = PersistStateStep::new(mem.clone());

        let mut ctx = PipelineContext::new("agent-1", "conv-1");
        ctx.messages.push(Message::system("system prompt"));
        ctx.messages.push(Message::user("hi"));

        step.execute(&mut ctx).await.unwrap();

        // Only the user message should be saved.
        assert_eq!(mem.saved_conversations().len(), 1);
    }
}
