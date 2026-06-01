//! RetrieveContextStep — retrieves RAG context and assembles the system prompt.
//!
//! Responsibilities:
//! - Extracts the user query from the latest message.
//! - Calls the memory RAG pipeline to retrieve relevant context snippets.
//! - Assembles a system prompt combining the agent's base system prompt with the
//!   retrieved context, trimming to the context-window budget if needed.
//!
//! # Cross-module dependencies
//! - Reads from `clawz_core::traits::MemoryBackend` (conversation history).
//! - Writes metadata keys consumed by [`SelectProviderStep`](crate::runtime::steps::provider::SelectProviderStep).
//! - Uses `clawz_core::traits::PipelineStep` for integration into the pipeline.

use async_trait::async_trait;
// Dependency: core traits and types for pipeline integration.
use clawz_core::{
    error::Result,
    traits::{MemoryBackend, PipelineContext, PipelineStep, StepOutcome},
    types::message::Role,
};
use std::sync::Arc;

/// Characters-per-token approximation used for budget trimming.
///
/// OpenAI and most Llama-based models average ~4 chars per token for English
/// text.  This is a coarse heuristic; real tokenisers are model-specific.
const CHARS_PER_TOKEN: usize = 4;
/// Default context-window budget reserved for the system prompt (in tokens).
///
/// Leaves the majority of the context window for user/assistant messages.
const DEFAULT_SYSTEM_PROMPT_BUDGET_TOKENS: usize = 2048;
/// Maximum number of RAG snippets to retrieve.
///
/// Retrieving too many snippets can drown the model in noise; 5 is a
/// pragmatic balance between coverage and signal.
const MAX_RAG_SNIPPETS: usize = 5;

/// Metadata key under which the assembled system prompt is stored.
///
/// Dependency: consumed by [`SelectProviderStep`](crate::runtime::steps::provider::SelectProviderStep)
/// when building the final `ChatRequest`.
pub const META_SYSTEM_PROMPT: &str = "system_prompt";
/// Metadata key for the number of context snippets injected.
pub const META_CONTEXT_SNIPPETS: &str = "context_snippets";
/// Metadata key listing workspace skill names injected this turn.
pub const META_WORKSPACE_SKILLS: &str = "workspace_skills";

/// Pipeline step that retrieves conversation history and assembles the system prompt.
///
/// This step runs early in the pipeline so that downstream steps (especially
/// `SelectProviderStep`) have a fully-formed system message available.
pub struct RetrieveContextStep {
    /// Memory backend for RAG retrieval and conversation history.
    memory: Arc<dyn MemoryBackend>,
    /// Base system prompt from the agent config (e.g. "You are a helpful assistant").
    base_system_prompt: String,
    /// Maximum number of tokens for the assembled system prompt.
    budget_tokens: usize,
    /// Optional operator workspace (`AGENTS.md`, skills).
    workspace: Option<Arc<crate::workspace::WorkspaceLoader>>,
}

impl RetrieveContextStep {
    /// Create a new step with the given memory backend and base prompt.
    pub fn new(memory: Arc<dyn MemoryBackend>, base_system_prompt: impl Into<String>) -> Self {
        Self {
            memory,
            base_system_prompt: base_system_prompt.into(),
            budget_tokens: DEFAULT_SYSTEM_PROMPT_BUDGET_TOKENS,
            workspace: None,
        }
    }

    /// Merge `AGENTS.md` and `skills/*/SKILL.md` from the operator workspace.
    pub fn with_workspace(mut self, loader: Arc<crate::workspace::WorkspaceLoader>) -> Self {
        self.workspace = Some(loader);
        self
    }

    /// Override the token budget for the assembled system prompt.
    pub fn with_budget(mut self, tokens: usize) -> Self {
        self.budget_tokens = tokens;
        self
    }

    /// Extract the text of the latest user message, if any.
    ///
    /// Scans backwards because the newest user message is the current query
    /// that needs RAG context.
    fn extract_query(ctx: &PipelineContext) -> Option<String> {
        ctx.messages.iter().rev().find_map(|m| {
            if m.role == Role::User {
                m.content.as_text().map(|t| t.to_string())
            } else {
                None
            }
        })
    }

    /// Trim `text` so it fits within `max_tokens`.
    ///
    /// Uses a simple char-count heuristic (4 chars ≈ 1 token).  If truncation
    /// is needed we try to break at a word boundary so the prompt remains
    /// readable.
    fn trim_to_budget(text: &str, max_tokens: usize) -> String {
        let max_chars = max_tokens * CHARS_PER_TOKEN;
        if text.len() <= max_chars {
            text.to_string()
        } else {
            // Trim at a word boundary if possible so we don't split tokens.
            let truncated = &text[..max_chars];
            match truncated.rfind(' ') {
                Some(pos) => format!("{}…", &truncated[..pos]),
                None => format!("{truncated}…"),
            }
        }
    }
}

#[async_trait]
impl PipelineStep for RetrieveContextStep {
    fn name(&self) -> &str {
        "retrieve_context"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome> {
        let query = Self::extract_query(ctx).unwrap_or_default();

        // Retrieve relevant history snippets from memory.
        // We use conversation history as a lightweight RAG source; a future
        // enhancement could also call memory.search() with an embedding.
        let history = self
            .memory
            .get_conversation_history(&ctx.conversation_id, MAX_RAG_SNIPPETS)
            .await
            .unwrap_or_default();

        // Build context block from recent history (exclude the very last message
        // which is already in ctx.messages).
        let mut fts_snippets = Vec::new();
        if !query.is_empty() {
            fts_snippets = self
                .memory
                .search_text(&ctx.agent_id, &query, MAX_RAG_SNIPPETS)
                .await
                .unwrap_or_default();
        }

        let mut tree_block = String::new();
        if !query.is_empty() {
            if let Ok(tree) = crate::memory::MemoryTree::global().await {
                if let Ok(nodes) = tree.search(&ctx.agent_id, &query, MAX_RAG_SNIPPETS).await {
                    tree_block = crate::memory::MemoryTree::format_context(&nodes);
                }
            }
        }

        let snippets_count = history.len() + fts_snippets.len();
        let mut context_block = if history.is_empty() && fts_snippets.is_empty() {
            String::new()
        } else {
            let mut block = String::from("\n\n## Relevant Context\n");
            for msg in &history {
                let role_label = match msg.role {
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                    Role::System => "System",
                    Role::Tool => "Tool",
                };
                if let Some(text) = msg.content.as_text() {
                    block.push_str(&format!("**{role_label}**: {text}\n"));
                }
            }
            for entry in &fts_snippets {
                let body = entry
                    .value
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| entry.value.to_string());
                block.push_str(&format!("**Memory ({})**: {body}\n", entry.key));
            }
            block
        };
        context_block.push_str(&tree_block);

        let workspace_block = if let Some(loader) = &self.workspace {
            match loader.load_snapshot() {
                Ok(snap) => {
                    let names: Vec<_> = snap.skills.iter().map(|s| s.name.clone()).collect();
                    ctx.insert_meta(
                        META_WORKSPACE_SKILLS,
                        serde_json::Value::Array(
                            names
                                .iter()
                                .map(|n| serde_json::Value::String(n.clone()))
                                .collect(),
                        ),
                    );
                    crate::workspace::WorkspaceLoader::build_skills_prompt_snapshot(&snap)
                }
                Err(e) => {
                    tracing::warn!("workspace load failed: {e}");
                    String::new()
                }
            }
        } else {
            String::new()
        };

        // Assemble full system prompt.
        let raw_prompt = if context_block.is_empty() && workspace_block.is_empty() {
            self.base_system_prompt.clone()
        } else {
            format!(
                "{}{}{}",
                self.base_system_prompt, workspace_block, context_block
            )
        };

        // Trim to budget so we never exceed the reserved system-prompt window.
        let system_prompt = Self::trim_to_budget(&raw_prompt, self.budget_tokens);

        ctx.insert_meta(META_SYSTEM_PROMPT, serde_json::Value::String(system_prompt));
        ctx.insert_meta(
            META_CONTEXT_SNIPPETS,
            serde_json::Value::Number(snippets_count.into()),
        );

        Ok(StepOutcome::Continue)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::message::Message;

    struct NullMemory;

    #[async_trait]
    impl MemoryBackend for NullMemory {
        async fn store(
            &self,
            _: &str,
            _: &str,
            _: serde_json::Value,
            _: Option<Vec<f32>>,
        ) -> Result<()> {
            Ok(())
        }
        async fn retrieve(&self, _: &str, _: &str) -> Result<Option<serde_json::Value>> {
            Ok(None)
        }
        async fn search(
            &self,
            _: &str,
            _: Vec<f32>,
            _: usize,
        ) -> Result<Vec<clawz_core::traits::MemoryEntry>> {
            Ok(vec![])
        }
        async fn get_conversation_history(&self, _: &str, _: usize) -> Result<Vec<Message>> {
            Ok(vec![])
        }
        async fn save_message(&self, _: &str, _: &Message) -> Result<()> {
            Ok(())
        }
        async fn delete(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_system_prompt_stored_in_metadata() {
        let memory = Arc::new(NullMemory);
        let step = RetrieveContextStep::new(memory, "You are a helpful assistant.");
        let mut ctx = PipelineContext::new("agent-1", "conv-1");
        ctx.messages.push(Message::user("Hello"));

        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));

        let prompt = ctx.get_meta(META_SYSTEM_PROMPT).unwrap();
        assert!(
            prompt
                .as_str()
                .unwrap()
                .contains("You are a helpful assistant")
        );
    }

    #[tokio::test]
    async fn test_trim_to_budget() {
        let long = "a".repeat(100);
        let trimmed = RetrieveContextStep::trim_to_budget(&long, 10); // 10 * 4 = 40 chars
        // 40 ascii chars + "…" (3 UTF-8 bytes) = 43 bytes
        assert!(trimmed.len() <= 44);
        assert!(trimmed.ends_with('…'));
    }

    #[tokio::test]
    async fn test_workspace_in_system_prompt() {
        let dir = std::env::temp_dir().join(format!("clawz-ctx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("skills/ws-skill")).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Workspace rules apply.").unwrap();
        std::fs::write(dir.join("skills/ws-skill/SKILL.md"), "# S\n\nDo X.").unwrap();

        let memory = Arc::new(NullMemory);
        let step = RetrieveContextStep::new(memory, "Base prompt.")
            .with_workspace(Arc::new(crate::workspace::WorkspaceLoader::new(&dir)));
        let mut ctx = PipelineContext::new("agent-1", "conv-1");
        ctx.messages.push(Message::user("Hi"));

        step.execute(&mut ctx).await.unwrap();
        let prompt = ctx.get_meta(META_SYSTEM_PROMPT).unwrap().as_str().unwrap();
        assert!(prompt.contains("Base prompt"));
        assert!(prompt.contains("Workspace rules"));
        assert!(prompt.contains("ws-skill"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
