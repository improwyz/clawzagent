//! AgentRuntime — the main agent execution loop.
//!
//! Composes the pipeline steps into a complete request-response cycle and
//! provides both single-turn and multi-turn conversation interfaces.
//!
//! # Responsibilities
//! 1. **Pipeline construction** — assembles the default step sequence
//!    (receive → retrieve context → select provider → execute tools →
//!    apply governance → persist state → stream response).
//! 2. **Single-turn execution** — [`AgentRuntime::run`] runs one user message
//!    through the pipeline and returns the assistant reply.
//! 3. **Multi-turn execution** — [`AgentRuntime::run_multi_turn`] loops until
//!    the conversation naturally ends, the cost budget is exhausted, or the
//!    maximum turn count is reached.
//! 4. **Cost budget enforcement** — hard-caps spend per conversation; halts
//!    gracefully with an explanatory message when the limit is hit.
//!
//! # Cross-module wiring
//! - Depends on `steps::*` for every [`PipelineStep`] implementation.
//! - Depends on `crate::providers` for LLM routing and cost tracking.
//! - Persists state via `clawz_core::traits::MemoryBackend`.
//! - Governance checks via `clawz_core::traits::GovernanceEngine`.

use std::sync::Arc;

// Dependency: core error and trait definitions from the shared `clawz_core` crate.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{GovernanceEngine, MemoryBackend, PipelineContext, PipelineStep, StepOutcome},
    types::{
        agent::{AgentConfig, AgentState, AgentStatus},
        message::{Message, MessageContent, Role},
    },
};

// Dependency: provider routing and cost tracking from the worker's provider module.
use crate::providers::{cost::CostTracker, router::ProviderRouter};
// Dependency: pipeline builder and the concrete step implementations in the `steps` submodule.
use crate::runtime::{
    pipeline::{Pipeline, PipelineBuilder},
    steps::{
        ApplyGovernanceStep, ExecuteToolsStep, PersistStateStep, ReceiveMessageStep,
        RetrieveContextStep, StreamResponseStep,
    },
};

/// Default maximum turns for a multi-turn conversation.
///
/// This guard prevents runaway tool-call loops or models that refuse to
/// emit a `finish_reason=stop`. Chosen as a sensible trade-off between
/// allowing complex reasoning and avoiding infinite spend.
const DEFAULT_MAX_TURNS: usize = 20;
/// Default per-conversation cost budget in USD.
///
/// Can be overridden per-runtime via [`AgentRuntime::with_cost_budget`].
/// The default is intentionally conservative for safety.
const DEFAULT_COST_BUDGET_USD: f64 = 1.0;

/// All external dependencies injected into the runtime.
///
/// Keeps `AgentRuntime` decoupled from concrete backends so tests can
/// substitute stubs without changing production code.
pub struct RuntimeDependencies {
    /// Routes chat requests to the correct LLM provider (OpenAI, Anthropic, local, etc.).
    pub provider_router: Arc<ProviderRouter>,
    /// Conversation history, RAG retrieval, and agent-state persistence.
    pub memory: Arc<dyn MemoryBackend>,
    /// Policy evaluation engine for trust scores and approval workflows.
    pub governance: Arc<dyn GovernanceEngine>,
    /// Tracks per-request and cumulative spend; feeds into `CostRepo` in `clawz_core::db`.
    pub cost_tracker: Arc<CostTracker>,

    // ---- Optional governance + scaling components (populated per deployment mode) ----
    /// Human-in-the-loop approval workflow (Tier-2/3 modes).
    pub approval_workflow: Option<Arc<crate::governance::approval::ApprovalWorkflow>>,
    /// Multi-agent deliberation council for high-risk proposals.
    pub council: Option<Arc<crate::governance::council::Council>>,
    /// Tamper-evident SHA-256 hash-chained audit logger.
    pub audit_logger: Option<Arc<crate::governance::audit::AuditLogger>>,
    /// Trust scorer for agents and skills.
    pub trust_scorer: Option<Arc<crate::governance::trust::TrustScorer>>,
    /// Routing bridge between the improvement pipeline and mode-appropriate governance.
    pub proposal_gatekeeper: Option<Arc<crate::governance::proposal_gate::ProposalGatekeeper>>,
    /// Skill repository for skill.md / soul.md / agents.md lifecycle management.
    pub skill_repository: Option<Arc<dyn crate::governance::skill_repository::SkillRepository>>,
    /// Cascading agent-tree spawner with auto-scale.
    pub spawner: Option<Arc<crate::runtime::spawner::AgentTreeSpawner>>,
    /// FSM-based deployment mode transitions and elastic scaling.
    pub elasticity: Option<Arc<crate::deployment::elasticity::DeploymentElasticity>>,
}

impl RuntimeDependencies {
    /// Builder-style method to set approval_workflow
    pub fn with_approval_workflow(mut self, w: Arc<crate::governance::approval::ApprovalWorkflow>) -> Self {
        self.approval_workflow = Some(w); self
    }
    /// Builder-style method to set council
    pub fn with_council(mut self, c: Arc<crate::governance::council::Council>) -> Self {
        self.council = Some(c); self
    }
    /// Builder-style method to set audit_logger
    pub fn with_audit_logger(mut self, a: Arc<crate::governance::audit::AuditLogger>) -> Self {
        self.audit_logger = Some(a); self
    }
    /// Builder-style method to set trust_scorer
    pub fn with_trust_scorer(mut self, t: Arc<crate::governance::trust::TrustScorer>) -> Self {
        self.trust_scorer = Some(t); self
    }
    /// Builder-style method to set proposal_gatekeeper
    pub fn with_proposal_gatekeeper(mut self, p: Arc<crate::governance::proposal_gate::ProposalGatekeeper>) -> Self {
        self.proposal_gatekeeper = Some(p); self
    }
    /// Builder-style method to set skill_repository
    pub fn with_skill_repository(mut self, r: Arc<dyn crate::governance::skill_repository::SkillRepository>) -> Self {
        self.skill_repository = Some(r); self
    }
    /// Builder-style method to set spawner
    pub fn with_spawner(mut self, s: Arc<crate::runtime::spawner::AgentTreeSpawner>) -> Self {
        self.spawner = Some(s); self
    }
    /// Builder-style method to set elasticity
    pub fn with_elasticity(mut self, e: Arc<crate::deployment::elasticity::DeploymentElasticity>) -> Self {
        self.elasticity = Some(e); self
    }
}

/// Main agent execution engine.
///
/// Owns the agent's [`AgentConfig`] and a shared bag of [`RuntimeDependencies`].
/// Each call to `run` or `run_multi_turn` creates a fresh [`PipelineContext`],
/// ensuring conversations are isolated while reusing the same backends.
pub struct AgentRuntime {
    /// Static configuration for this agent (model, system prompt, id, etc.).
    config: AgentConfig,
    /// Shared, immutable dependency bag. Wrapped in `Arc` so the runtime
    /// can be cloned / moved across async boundaries cheaply.
    deps: Arc<RuntimeDependencies>,
    /// Hard limit on conversation turns. Prevents infinite loops.
    max_turns: usize,
    /// Hard limit on cumulative spend (USD). Prevents runaway costs.
    cost_budget_usd: f64,
}

impl AgentRuntime {
    /// Create a new runtime with the given config and dependencies.
    pub fn new(config: AgentConfig, deps: RuntimeDependencies) -> Self {
        Self {
            config,
            deps: Arc::new(deps),
            max_turns: DEFAULT_MAX_TURNS,
            cost_budget_usd: DEFAULT_COST_BUDGET_USD,
        }
    }

    /// Override the maximum turn limit.
    pub fn with_max_turns(mut self, n: usize) -> Self {
        self.max_turns = n;
        self
    }

    /// Override the per-conversation cost budget (USD).
    pub fn with_cost_budget(mut self, usd: f64) -> Self {
        self.cost_budget_usd = usd;
        self
    }

    /// Build the standard pipeline for a single turn.
    ///
    /// The step order is deliberate:
    /// 1. **ReceiveMessage** — inject the incoming message into context.
    /// 2. **RetrieveContext** — RAG retrieval + system-prompt assembly.
    /// 3. **SelectProvider** — route to LLM, record cost, append assistant response.
    /// 4. **ExecuteTools** — run any tool_calls in the assistant message.
    /// 5. **ApplyGovernance** — policy check; may substitute a safe decline message.
    /// 6. **PersistState** — save messages and agent state to memory backend.
    /// 7. **StreamResponse** — yield the final response to the caller.
    fn build_pipeline(&self) -> Pipeline {
        let retrieve = RetrieveContextStep::new(
            self.deps.memory.clone(),
            self.config.system_prompt.clone(),
        );

        let provider = crate::runtime::steps::provider::SelectProviderStep::new(
            self.deps.provider_router.clone(),
            self.deps.cost_tracker.clone(),
            self.config.model.clone(),
        );

        let tools_step = ExecuteToolsStep::new(
            self.config.id.to_string(),
            "conv-placeholder", // replaced at runtime by ctx.conversation_id
        );

        let governance = ApplyGovernanceStep::new(
            self.deps.governance.clone(),
            "agent_chat",
        );

        let persist = PersistStateStep::new(self.deps.memory.clone());

        let stream = StreamResponseStep::new();

        PipelineBuilder::new("agent_pipeline")
            .step(ReceiveMessageStep::new())
            .step(retrieve)
            .step(provider)
            .step(tools_step)
            .step(governance)
            .step(persist)
            .step(stream)
            .build()
    }

    /// Execute the pipeline for a single turn.
    ///
    /// `message` is appended to `ctx.messages` before running.
    /// This is the inner primitive used by both `run` and `run_multi_turn`.
    async fn run_turn(
        &self,
        ctx: &mut PipelineContext,
        message: Message,
    ) -> Result<StepOutcome> {
        ctx.messages.push(message);
        let pipeline = self.build_pipeline();
        let result = pipeline.execute(ctx).await?;
        Ok(result.outcome)
    }

    /// Execute a full pipeline for a single user message.
    ///
    /// Returns the assistant's reply.  Creates a fresh conversation context,
    /// so this is suitable for stateless / one-shot use cases.
    pub async fn run(&self, message: Message) -> Result<Message> {
        let mut ctx = PipelineContext::new(
            self.config.id.to_string(),
            uuid::Uuid::new_v4().to_string(),
        );
        ctx.agent_state = AgentState::running("processing message");

        self.run_turn(&mut ctx, message).await?;

        // Return the last assistant message.
        // We scan from the back because the assistant reply is always appended
        // at the end of the message list by `SelectProviderStep`.
        ctx.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .cloned()
            .ok_or_else(|| ClawzError::Internal("pipeline produced no assistant message".into()))
    }

    /// Execute a multi-turn conversation.
    ///
    /// Returns the full list of messages including assistant replies.
    ///
    /// # Turn loop mechanics
    /// 1. Seed context with all but the last initial message.
    /// 2. Run the pipeline with the last (trigger) message.
    /// 3. Inspect the [`StepOutcome`]:
    ///    - `Continue` — if the assistant emitted tool calls, synthesise a
    ///      "(continue after tool execution)" message and loop; otherwise finish.
    ///    - `Halt` — stop immediately.
    ///    - `Delegate` — stop and let the caller handle delegation.
    /// 4. Before every turn, enforce the cost budget and max-turn guards.
    pub async fn run_multi_turn(
        &self,
        initial_messages: Vec<Message>,
    ) -> Result<Vec<Message>> {
        let conversation_id = uuid::Uuid::new_v4().to_string();
        let mut ctx =
            PipelineContext::new(self.config.id.to_string(), conversation_id.clone());
        ctx.agent_state = AgentState::running("multi-turn conversation");

        let mut turns = 0;

        // Seed context with prior messages (all except the last user message).
        // This lets the model see the full conversation history before the
        // current turn, matching the chat-completions API semantics.
        for msg in initial_messages.iter().take(initial_messages.len().saturating_sub(1)) {
            ctx.messages.push(msg.clone());
        }

        // The last message triggers the first turn.
        let first_message = initial_messages
            .last()
            .cloned()
            .ok_or_else(|| ClawzError::Validation("no messages provided".into()))?;

        let mut current_message = first_message;

        loop {
            // Budget guard — check *before* each turn so we never exceed the cap.
            if ctx.cost_accumulated >= self.cost_budget_usd {
                log::warn!(
                    "[agent_runtime] cost budget ${:.4} exceeded after {} turns",
                    self.cost_budget_usd,
                    turns
                );
                ctx.messages.push(Message::assistant(
                    "I've reached my cost budget limit for this conversation.",
                ));
                break;
            }

            // Max turns guard — prevents infinite tool-call loops.
            if turns >= self.max_turns {
                log::warn!(
                    "[agent_runtime] max turns ({}) reached",
                    self.max_turns
                );
                break;
            }

            ctx.agent_state.set_status(AgentStatus::Running);
            let outcome = self.run_turn(&mut ctx, current_message.clone()).await?;
            turns += 1;

            match outcome {
                StepOutcome::Continue => {
                    // Check if the last assistant message contains tool calls.
                    // If so, the pipeline injected tool results during this turn,
                    // and we need to re-enter the LLM so it can see those results
                    // and potentially emit more tool calls or a final text response.
                    let has_tool_calls = ctx
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m.role == Role::Assistant)
                        .map(|m| matches!(&m.content, MessageContent::ToolCalls(_)))
                        .unwrap_or(false);

                    if has_tool_calls && turns < self.max_turns {
                        // Build a synthetic "continue" message to re-enter the pipeline
                        // after tool results have been injected by ExecuteToolsStep.
                        // The content is arbitrary; the step only needs a trigger to loop.
                        current_message = Message::user("(continue after tool execution)");
                        continue;
                    }
                    // Normal completion — no further tool calls pending.
                    break;
                }
                StepOutcome::Halt => {
                    log::info!("[agent_runtime] pipeline halted after {} turns", turns);
                    break;
                }
                StepOutcome::Delegate { target_agent } => {
                    log::info!(
                        "[agent_runtime] delegating to '{}' after {} turns",
                        target_agent,
                        turns
                    );
                    break;
                }
            }
        }

        ctx.agent_state.set_status(AgentStatus::Idle);
        Ok(ctx.messages)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::{
        error::Result as ClawzResult,
        traits::{GovernanceEngine, MemoryBackend, MemoryEntry},
        types::{
            agent::AgentConfig,
            governance::{ApprovalRequest, GovernanceResult, TrustTier},
            message::Message,
        },
    };
    use std::sync::Mutex;

    // Stub memory backend — satisfies all trait methods with no-ops.
    struct StubMemory;
    #[async_trait::async_trait]
    impl MemoryBackend for StubMemory {
        async fn store(
            &self, _: &str, _: &str, _: serde_json::Value, _: Option<Vec<f32>>,
        ) -> ClawzResult<()> { Ok(()) }
        async fn retrieve(&self, _: &str, _: &str) -> ClawzResult<Option<serde_json::Value>> { Ok(None) }
        async fn search(&self, _: &str, _: Vec<f32>, _: usize) -> ClawzResult<Vec<MemoryEntry>> { Ok(vec![]) }
        async fn get_conversation_history(&self, _: &str, _: usize) -> ClawzResult<Vec<Message>> { Ok(vec![]) }
        async fn save_message(&self, _: &str, _: &Message) -> ClawzResult<()> { Ok(()) }
        async fn delete(&self, _: &str, _: &str) -> ClawzResult<()> { Ok(()) }
    }

    // Stub governance engine — always allows everything.
    struct StubGovernance;
    #[async_trait::async_trait]
    impl GovernanceEngine for StubGovernance {
        async fn evaluate(&self, _: &str, _: &str, _: &serde_json::Value) -> ClawzResult<GovernanceResult> {
            Ok(GovernanceResult::allow(0.8))
        }
        async fn get_trust_score(&self, _: &str) -> ClawzResult<f64> { Ok(0.8) }
        async fn update_trust(&self, _: &str, _: f64, _: &str) -> ClawzResult<()> { Ok(()) }
        async fn check_policy(&self, _: &str, _: &str) -> ClawzResult<bool> { Ok(true) }
        async fn request_approval(&self, _: ApprovalRequest) -> ClawzResult<String> {
            Ok(uuid::Uuid::new_v4().to_string())
        }
    }

    #[test]
    fn test_runtime_constructed() {
        let config = AgentConfig::new("test-agent", "gpt-4");
        let deps = RuntimeDependencies {
            // Building a runtime inside a sync test requires blocking on the async router init.
            provider_router: Arc::new(tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap()
                .block_on(ProviderRouter::new(crate::providers::ProviderRouterConfig::default()))
                .unwrap()),
            memory: Arc::new(StubMemory),
            governance: Arc::new(StubGovernance),
            cost_tracker: Arc::new(CostTracker::new()),
            approval_workflow: None,
            council: None,
            audit_logger: None,
            trust_scorer: None,
            proposal_gatekeeper: None,
            skill_repository: None,
            spawner: None,
            elasticity: None,
        };
        let rt = AgentRuntime::new(config, deps);
        assert_eq!(rt.max_turns, DEFAULT_MAX_TURNS);
    }
}
