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
    traits::{GovernanceEngine, MemoryBackend, PipelineContext, StepOutcome},
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
    /// Runtime capability registry for semantic tool discovery.
    pub capability_registry: Option<Arc<crate::tools::capability_registry::ToolCapabilityRegistry>>,
    /// Tracks task outcomes for the self-improvement loop and performance metrics.
    pub outcome_tracker: Option<Arc<crate::memory::outcome_tracker::OutcomeTracker>>,
    /// Analyzes task complexity to inform dynamic sub-agent spawning.
    pub complexity_analyzer: Option<Arc<crate::runtime::complexity::TaskComplexityAnalyzer>>,
    /// Environmental metrics (CPU, memory, queue depth) via Bollard Docker API.
    pub container_metrics: Option<Arc<crate::reality::container_metrics::ContainerMetrics>>,
    /// Peer-to-peer negotiation protocol for multi-round resource/task bargaining.
    pub negotiation_protocol: Option<Arc<crate::runtime::negotiation::NegotiationProtocol>>,
    /// Closed-loop self-improvement orchestrator (outcome → proposal → apply).
    pub self_improvement_loop: Option<Arc<crate::memory::improvement::SelfImprovementLoop>>,
    /// How often (in turns) to invoke the self-improvement loop. 0 = disabled.
    pub self_improvement_interval_turns: usize,
    /// Constitutional convention for agent-authored rule amendments and voting.
    pub constitution: Option<Arc<crate::governance::constitution::ConstitutionalConvention>>,
    /// Cross-session accumulated identity (trust, skills, experience).
    pub identity_store: Option<Arc<crate::runtime::identity::AgentIdentityStore>>,
    pub idempotency_store: Option<Arc<dyn clawz_core::traits::IdempotencyStore>>,
    /// MBTI drift detector — checks if observed behaviour no longer matches the seeded type.
    pub mbti_drift_detector: Option<Arc<crate::runtime::mbti_drift_detector::MBTIDriftDetector>>,
    /// Tool registry (source of truth); pipeline uses [`pipeline_tools`] snapshot.
    pub tool_registry: Option<Arc<crate::tools::registry::ToolRegistry>>,
    /// Tools copied from the registry when the runtime was constructed (sync pipeline build).
    pub pipeline_tools: Vec<Arc<dyn clawz_core::traits::Tool>>,
    /// JSON schemas passed to the provider on each turn.
    pub tool_schemas: Vec<clawz_core::types::tool::ToolSchema>,
    /// Optional broadcast bus for turn lifecycle events.
    pub turn_event_bus: Option<Arc<crate::runtime::turn_events::TurnEventBus>>,
    /// Operator workspace loader (`AGENTS.md`, skills).
    pub workspace_loader: Option<Arc<crate::workspace::WorkspaceLoader>>,
}

impl RuntimeDependencies {
    /// Construct with the four required dependencies. All optional components are
    /// left absent; use the builder-style `with_*` methods to attach them.
    pub fn new(
        provider_router: Arc<ProviderRouter>,
        memory: Arc<dyn MemoryBackend>,
        governance: Arc<dyn GovernanceEngine>,
        cost_tracker: Arc<CostTracker>,
    ) -> Self {
        Self {
            provider_router,
            memory,
            governance,
            cost_tracker,
            approval_workflow: None,
            council: None,
            audit_logger: None,
            trust_scorer: None,
            proposal_gatekeeper: None,
            skill_repository: None,
            spawner: None,
            elasticity: None,
            capability_registry: None,
            outcome_tracker: None,
            complexity_analyzer: None,
            container_metrics: None,
            negotiation_protocol: None,
            self_improvement_loop: None,
            self_improvement_interval_turns: 0,
            constitution: None,
            identity_store: None,
            idempotency_store: None,
            mbti_drift_detector: None,
            tool_registry: None,
            pipeline_tools: Vec::new(),
            tool_schemas: Vec::new(),
            turn_event_bus: None,
            workspace_loader: None,
        }
    }

    /// Attach a tool registry snapshot for pipeline execution and provider schemas.
    pub fn with_tooling(
        mut self,
        registry: Arc<crate::tools::registry::ToolRegistry>,
        pipeline_tools: Vec<Arc<dyn clawz_core::traits::Tool>>,
        tool_schemas: Vec<clawz_core::types::tool::ToolSchema>,
    ) -> Self {
        self.tool_registry = Some(registry);
        self.pipeline_tools = pipeline_tools;
        self.tool_schemas = tool_schemas;
        self
    }

    /// Attach a turn event bus for streaming tool/provider events.
    pub fn with_turn_event_bus(
        mut self,
        bus: Arc<crate::runtime::turn_events::TurnEventBus>,
    ) -> Self {
        self.turn_event_bus = Some(bus);
        self
    }

    pub fn with_workspace_loader(mut self, loader: Arc<crate::workspace::WorkspaceLoader>) -> Self {
        self.workspace_loader = Some(loader);
        self
    }

    /// Builder-style method to set provider_router
    pub fn with_provider_router(mut self, r: Arc<ProviderRouter>) -> Self {
        self.provider_router = r;
        self
    }

    /// Builder-style method to set memory
    pub fn with_memory(mut self, m: Arc<dyn MemoryBackend>) -> Self {
        self.memory = m;
        self
    }

    /// Builder-style method to set governance
    pub fn with_governance(mut self, g: Arc<dyn GovernanceEngine>) -> Self {
        self.governance = g;
        self
    }

    /// Builder-style method to set cost_tracker
    pub fn with_cost_tracker(mut self, c: Arc<CostTracker>) -> Self {
        self.cost_tracker = c;
        self
    }

    /// Builder-style method to set approval_workflow
    pub fn with_approval_workflow(
        mut self,
        w: Arc<crate::governance::approval::ApprovalWorkflow>,
    ) -> Self {
        self.approval_workflow = Some(w);
        self
    }
    /// Builder-style method to set council
    pub fn with_council(mut self, c: Arc<crate::governance::council::Council>) -> Self {
        self.council = Some(c);
        self
    }
    /// Builder-style method to set audit_logger
    pub fn with_audit_logger(mut self, a: Arc<crate::governance::audit::AuditLogger>) -> Self {
        self.audit_logger = Some(a);
        self
    }
    /// Builder-style method to set trust_scorer
    pub fn with_trust_scorer(mut self, t: Arc<crate::governance::trust::TrustScorer>) -> Self {
        self.trust_scorer = Some(t);
        self
    }
    /// Builder-style method to set proposal_gatekeeper
    pub fn with_proposal_gatekeeper(
        mut self,
        p: Arc<crate::governance::proposal_gate::ProposalGatekeeper>,
    ) -> Self {
        self.proposal_gatekeeper = Some(p);
        self
    }
    /// Builder-style method to set skill_repository
    pub fn with_skill_repository(
        mut self,
        r: Arc<dyn crate::governance::skill_repository::SkillRepository>,
    ) -> Self {
        self.skill_repository = Some(r);
        self
    }
    /// Builder-style method to set spawner
    pub fn with_spawner(mut self, s: Arc<crate::runtime::spawner::AgentTreeSpawner>) -> Self {
        self.spawner = Some(s);
        self
    }
    /// Builder-style method to set elasticity
    pub fn with_elasticity(
        mut self,
        e: Arc<crate::deployment::elasticity::DeploymentElasticity>,
    ) -> Self {
        self.elasticity = Some(e);
        self
    }
    pub fn with_identity_store(
        mut self,
        s: Arc<crate::runtime::identity::AgentIdentityStore>,
    ) -> Self {
        self.identity_store = Some(s);
        self
    }
    pub fn with_idempotency_store(
        mut self,
        s: Arc<dyn clawz_core::traits::IdempotencyStore>,
    ) -> Self {
        self.idempotency_store = Some(s);
        self
    }
    pub fn with_outcome_tracker(
        mut self,
        t: Arc<crate::memory::outcome_tracker::OutcomeTracker>,
    ) -> Self {
        self.outcome_tracker = Some(t);
        self
    }
    pub fn with_complexity_analyzer(
        mut self,
        a: Arc<crate::runtime::complexity::TaskComplexityAnalyzer>,
    ) -> Self {
        self.complexity_analyzer = Some(a);
        self
    }
    pub fn with_container_metrics(
        mut self,
        m: Arc<crate::reality::container_metrics::ContainerMetrics>,
    ) -> Self {
        self.container_metrics = Some(m);
        self
    }
    pub fn with_negotiation_protocol(
        mut self,
        n: Arc<crate::runtime::negotiation::NegotiationProtocol>,
    ) -> Self {
        self.negotiation_protocol = Some(n);
        self
    }
    pub fn with_self_improvement_loop(
        mut self,
        l: Arc<crate::memory::improvement::SelfImprovementLoop>,
    ) -> Self {
        self.self_improvement_loop = Some(l);
        self
    }
    pub fn with_self_improvement_interval(mut self, n: usize) -> Self {
        self.self_improvement_interval_turns = n;
        self
    }
    pub fn with_constitution(
        mut self,
        c: Arc<crate::governance::constitution::ConstitutionalConvention>,
    ) -> Self {
        self.constitution = Some(c);
        self
    }
    /// Builder-style method to set mbti_drift_detector
    pub fn with_mbti_drift_detector(
        mut self,
        d: Arc<crate::runtime::mbti_drift_detector::MBTIDriftDetector>,
    ) -> Self {
        self.mbti_drift_detector = Some(d);
        self
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

    /// Shared memory backend (conversation history, RAG, agent state).
    pub fn memory(&self) -> Arc<dyn MemoryBackend> {
        self.deps.memory.clone()
    }

    pub fn cost_tracker(&self) -> Arc<CostTracker> {
        self.deps.cost_tracker.clone()
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
    fn seed_tool_schemas(&self, ctx: &mut PipelineContext) {
        if !self.deps.tool_schemas.is_empty() {
            if let Ok(value) = serde_json::to_value(&self.deps.tool_schemas) {
                ctx.insert_meta("tool_schemas", value);
            }
        }
    }

    fn build_pipeline(&self, conversation_id: &str) -> Pipeline {
        let mut retrieve =
            RetrieveContextStep::new(self.deps.memory.clone(), self.config.system_prompt.clone());
        if let Some(ws) = &self.deps.workspace_loader {
            retrieve = retrieve.with_workspace(ws.clone());
        }

        let provider = crate::runtime::steps::provider::SelectProviderStep::new(
            self.deps.provider_router.clone(),
            self.deps.cost_tracker.clone(),
            self.config.model.clone(),
        );

        let mut tools_step = ExecuteToolsStep::new(self.config.id.to_string(), conversation_id);
        for tool in &self.deps.pipeline_tools {
            tools_step.register_tool(tool.clone());
        }
        if let Some(bus) = &self.deps.turn_event_bus {
            tools_step = tools_step.with_turn_event_bus(bus.clone());
        }

        let governance = ApplyGovernanceStep::new(self.deps.governance.clone(), "agent_chat");

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
    async fn run_turn(&self, ctx: &mut PipelineContext, message: Message) -> Result<StepOutcome> {
        self.seed_tool_schemas(ctx);
        ctx.messages.push(message);
        let pipeline = self.build_pipeline(&ctx.conversation_id);
        let result = pipeline.execute(ctx).await?;

        // Record outcome for the self-improvement loop.
        if let Some(ref tracker) = self.deps.outcome_tracker {
            let outcome = match &result.outcome {
                StepOutcome::Continue => crate::memory::outcome_tracker::TaskOutcome::Success,
                StepOutcome::Halt => crate::memory::outcome_tracker::TaskOutcome::Failure,
                StepOutcome::Delegate { .. } => {
                    crate::memory::outcome_tracker::TaskOutcome::Success
                }
            };
            let _ = tracker.record(&ctx.conversation_id, outcome).await;
        }

        // Record environmental metrics for reality awareness.
        if let Some(ref _metrics) = self.deps.container_metrics {
            if let Ok(m) =
                crate::reality::container_metrics::ContainerMetrics::fetch(&ctx.agent_id).await
            {
                log::debug!(
                    "[agent_runtime] container {} — cpu={:.1}%, mem={:.1}%, queue={}",
                    m.container_id,
                    m.cpu_percent,
                    m.memory_percent,
                    m.queue_depth
                );
            }
        }

        Ok(result.outcome)
    }

    /// Execute a full pipeline for a single user message.
    ///
    /// Returns the assistant's reply.  Creates a fresh conversation context,
    /// so this is suitable for stateless / one-shot use cases.
    pub async fn run(&self, message: Message) -> Result<Message> {
        self.run_in_conversation(message, uuid::Uuid::new_v4().to_string())
            .await
    }

    /// Execute a single turn scoped to an existing conversation (e.g. a room thread).
    pub async fn run_in_conversation(
        &self,
        message: Message,
        conversation_id: impl Into<String>,
    ) -> Result<Message> {
        self.run_in_conversation_with_meta(message, conversation_id, serde_json::Value::Null)
            .await
    }

    /// Execute a single turn with extra metadata forwarded into the pipeline context.
    pub async fn run_in_conversation_with_meta(
        &self,
        message: Message,
        conversation_id: impl Into<String>,
        meta: serde_json::Value,
    ) -> Result<Message> {
        let mut ctx = PipelineContext::new(self.config.id.to_string(), conversation_id);
        ctx.agent_state = AgentState::running("processing message");
        if let Some(obj) = meta.as_object() {
            for (key, value) in obj {
                ctx.insert_meta(key.clone(), value.clone());
            }
        }

        self.seed_tool_schemas(&mut ctx);
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
    pub async fn run_multi_turn(&self, initial_messages: Vec<Message>) -> Result<Vec<Message>> {
        let conversation_id = uuid::Uuid::new_v4().to_string();
        self.run_multi_turn_in_conversation(initial_messages, conversation_id)
            .await
    }

    /// Multi-turn loop scoped to a stable conversation id (sessions, channels, REST).
    pub async fn run_multi_turn_in_conversation(
        &self,
        initial_messages: Vec<Message>,
        conversation_id: impl Into<String>,
    ) -> Result<Vec<Message>> {
        let conversation_id = conversation_id.into();
        let mut ctx = PipelineContext::new(self.config.id.to_string(), conversation_id.clone());
        self.seed_tool_schemas(&mut ctx);
        ctx.agent_state = AgentState::running("multi-turn conversation");

        let mut turns = 0;

        // Seed context with prior messages (all except the last user message).
        // This lets the model see the full conversation history before the
        // current turn, matching the chat-completions API semantics.
        for msg in initial_messages
            .iter()
            .take(initial_messages.len().saturating_sub(1))
        {
            ctx.messages.push(msg.clone());
        }

        // The last message triggers the first turn.
        let first_message = initial_messages
            .last()
            .cloned()
            .ok_or_else(|| ClawzError::Validation("no messages provided".into()))?;

        let mut current_message = first_message;

        // Load and persist identity before/after the multi-turn session.
        let agent_id = self.config.id.to_string();
        if let Some(ref identity_store) = self.deps.identity_store {
            if let Ok(mut identity) = identity_store.load(&agent_id).await {
                identity.increment_session();
                let _ = identity_store.save(&identity).await;
            }
        }

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
                log::warn!("[agent_runtime] max turns ({}) reached", self.max_turns);
                break;
            }

            ctx.agent_state.set_status(AgentStatus::Running);

            // Analyze task complexity to inform spawning decisions.
            if let Some(ref analyzer) = self.deps.complexity_analyzer {
                if let Some(first_msg) = ctx.messages.first() {
                    if let Some(text) = first_msg.content.as_text() {
                        if let Ok(score) = analyzer.analyze(text).await {
                            log::debug!(
                                "[agent_runtime] complexity score: {:?} (parallelism_hint={})",
                                score.complexity,
                                score.parallelism_hint
                            );
                        }
                    }
                }
            }

            let outcome = self.run_turn(&mut ctx, current_message.clone()).await?;
            turns += 1;

            // Run self-improvement loop periodically.
            if self.deps.self_improvement_interval_turns > 0
                && turns % self.deps.self_improvement_interval_turns == 0
            {
                if let Some(ref loop_) = self.deps.self_improvement_loop {
                    log::debug!("[agent_runtime] self-improvement at turn {turns}");
                    if let Ok(changes) = loop_.run_once().await {
                        for change in &changes {
                            log::info!("[agent_runtime] applied: {change:?}");
                        }
                    }
                }
            }

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
                    log::info!("[agent_runtime] pipeline halted after {turns} turns");
                    break;
                }
                StepOutcome::Delegate { target_agent } => {
                    log::info!(
                        "[agent_runtime] delegating to '{target_agent}' after {turns} turns"
                    );
                    break;
                }
            }
        }

        ctx.agent_state.set_status(AgentStatus::Idle);

        // Persist identity after the session.
        if let Some(ref identity_store) = self.deps.identity_store {
            if let Ok(mut identity) = identity_store.load(&agent_id).await {
                // MBTI drift detection — check if observed behaviour no longer matches seeded type.
                if let Some(ref detector) = self.deps.mbti_drift_detector {
                    if let Some(drift_label) = detector.detect_drift(&identity) {
                        identity.state.mbti_drift_label = Some(drift_label);
                        let label_str = identity.state.mbti_drift_label.as_ref().unwrap().as_str();
                        log::info!(
                            "[agent_runtime] MBTI drift detected: {} -> {}",
                            identity.core.original_mbti.as_str(),
                            label_str
                        );
                        // Trigger self-healing: checkpoint current state when drift exceeds threshold
                        if identity.drift_score() > 0.75 {
                            let checkpoint_id = identity.compute_identity_version_hash();
                            let checkpoint_path = std::path::PathBuf::from(format!(
                                "/tmp/drift_checkpoint_{agent_id}.json"
                            ));
                            let checkpoint = serde_json::json!({
                                "agent_id": agent_id,
                                "identity_version_hash": checkpoint_id,
                                "timestamp": chrono::Utc::now().to_rfc3339(),
                            });
                            if let Some(parent) = checkpoint_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(
                                &checkpoint_path,
                                serde_json::to_string_pretty(&checkpoint).unwrap(),
                            );
                            log::warn!(
                                "[agent_runtime] identity drift checkpoint written (score={:.2})",
                                identity.drift_score()
                            );
                            // Emit governance event
                            let _ = identity
                                .emit(crate::runtime::identity::GovernanceEvent::IdentityDrift {
                                    drift_score: identity.drift_score(),
                                    checkpoint_id: checkpoint_path,
                                })
                                .await;
                        }
                    }
                }
                let _ = identity_store.save(&identity).await;
            }
        }

        Ok(ctx.messages)
    }

    /// Read-only accessor for the underlying [`AgentConfig`].
    ///
    /// Provided so callers (e.g. the gateway autonomous endpoint) can inspect
    /// the static configuration of the agent — name, model, system_prompt —
    /// without taking ownership of the runtime.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Get the capability registry for semantic tool discovery.
    ///
    /// Returns the capability registry if one was configured in
    /// [`RuntimeDependencies`], enabling agents to discover tools by
    /// keyword query at runtime.
    pub fn get_capabilities(
        &self,
    ) -> Option<Arc<crate::tools::capability_registry::ToolCapabilityRegistry>> {
        self.deps.capability_registry.clone()
    }

    /// Get the negotiation protocol for P2P resource/task bargaining.
    ///
    /// Returns the protocol if one was configured in [`RuntimeDependencies`],
    /// enabling agents to initiate multi-round negotiations during sessions.
    pub fn get_negotiation_protocol(
        &self,
    ) -> Option<Arc<crate::runtime::negotiation::NegotiationProtocol>> {
        self.deps.negotiation_protocol.clone()
    }

    /// Get the constitutional convention for agent-authored rule amendments.
    pub fn get_constitution(
        &self,
    ) -> Option<Arc<crate::governance::constitution::ConstitutionalConvention>> {
        self.deps.constitution.clone()
    }

    /// Start a long-running autonomous session for this agent.
    ///
    /// Constructs an [`AutonomousSession`] that records the per-session budgets
    /// (max turns and cumulative cost) and the current execution status. The
    /// session is created in the `Running` state; further turns and cost
    /// accumulation are recorded by the caller as the multi-turn loop advances.
    ///
    /// # Arguments
    /// - `agent_id` — the ID of the agent this session belongs to.
    /// - `max_turns` — optional override for the runtime's default max-turn
    ///   limit. When `None`, the runtime default is used.
    /// - `cost_budget_usd` — optional override for the per-session USD budget.
    ///   When `None`, the runtime default is used.
    ///
    /// # Errors
    /// Returns [`ClawzError::Validation`] when the provided `agent_id` does not
    /// match the runtime's configured agent.
    pub async fn start_autonomous_session(
        &self,
        agent_id: &str,
        max_turns: Option<usize>,
        cost_budget_usd: Option<f64>,
    ) -> Result<AutonomousSession> {
        if agent_id != self.config.id.to_string() {
            return Err(ClawzError::Validation(format!(
                "agent_id {} does not match runtime agent {}",
                agent_id, self.config.id
            )));
        }
        Ok(AutonomousSession {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            max_turns: max_turns.unwrap_or(self.max_turns),
            cost_budget_usd: cost_budget_usd.unwrap_or(self.cost_budget_usd),
            turns_executed: 0,
            cost_accumulated_usd: 0.0,
            status: AutonomousSessionStatus::Running,
        })
    }
}

/// Lifecycle states for an [`AutonomousSession`].
///
/// Sessions begin as [`AutonomousSessionStatus::Running`] and progress to one
/// of the terminal states as the multi-turn loop unfolds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomousSessionStatus {
    /// Session is actively executing turns.
    Running,
    /// Session reached its `max_turns` ceiling.
    MaxTurnsReached,
    /// Session reached its `cost_budget_usd` ceiling.
    BudgetExhausted,
    /// Session completed normally (model emitted a stop signal).
    Completed,
    /// Session was cancelled by a caller (e.g. via a stop endpoint).
    Cancelled,
}

/// A long-running, multi-turn agent execution context.
///
/// Returned by [`AgentRuntime::start_autonomous_session`]. The struct tracks
/// per-session budgets and current progress; consumers stream activity events
/// over the WebSocket `/ws/agents/{id}/stream` channel and read terminal state
/// here when the loop ends.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AutonomousSession {
    /// Stable session identifier (UUID-v4).
    pub id: String,
    /// Owning agent ID — matches [`AgentConfig::id`].
    pub agent_id: String,
    /// Hard upper bound on conversation turns for this session.
    pub max_turns: usize,
    /// Hard upper bound on accumulated USD spend for this session.
    pub cost_budget_usd: f64,
    /// Number of turns executed so far.
    pub turns_executed: usize,
    /// Cumulative USD spent across all turns.
    pub cost_accumulated_usd: f64,
    /// Current lifecycle status.
    pub status: AutonomousSessionStatus,
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
            governance::{ApprovalRequest, GovernanceResult},
            message::Message,
        },
    };

    // Stub memory backend — satisfies all trait methods with no-ops.
    struct StubMemory;
    #[async_trait::async_trait]
    impl MemoryBackend for StubMemory {
        async fn store(
            &self,
            _: &str,
            _: &str,
            _: serde_json::Value,
            _: Option<Vec<f32>>,
        ) -> ClawzResult<()> {
            Ok(())
        }
        async fn retrieve(&self, _: &str, _: &str) -> ClawzResult<Option<serde_json::Value>> {
            Ok(None)
        }
        async fn search(&self, _: &str, _: Vec<f32>, _: usize) -> ClawzResult<Vec<MemoryEntry>> {
            Ok(vec![])
        }
        async fn get_conversation_history(&self, _: &str, _: usize) -> ClawzResult<Vec<Message>> {
            Ok(vec![])
        }
        async fn save_message(&self, _: &str, _: &Message) -> ClawzResult<()> {
            Ok(())
        }
        async fn delete(&self, _: &str, _: &str) -> ClawzResult<()> {
            Ok(())
        }
    }

    // Stub governance engine — always allows everything.
    struct StubGovernance;
    #[async_trait::async_trait]
    impl GovernanceEngine for StubGovernance {
        async fn evaluate(
            &self,
            _: &str,
            _: &str,
            _: &serde_json::Value,
        ) -> ClawzResult<GovernanceResult> {
            Ok(GovernanceResult::allow(0.8))
        }
        async fn get_trust_score(&self, _: &str) -> ClawzResult<f64> {
            Ok(0.8)
        }
        async fn update_trust(&self, _: &str, _: f64, _: &str) -> ClawzResult<()> {
            Ok(())
        }
        async fn check_policy(&self, _: &str, _: &str) -> ClawzResult<bool> {
            Ok(true)
        }
        async fn request_approval(&self, _: ApprovalRequest) -> ClawzResult<String> {
            Ok(uuid::Uuid::new_v4().to_string())
        }
    }

    #[test]
    fn test_runtime_constructed() {
        let config = AgentConfig::new("test-agent", "gpt-4");
        let deps = RuntimeDependencies {
            // Building a runtime inside a sync test requires blocking on the async router init.
            provider_router: Arc::new(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(ProviderRouter::new(
                        crate::providers::ProviderRouterConfig::default(),
                    ))
                    .unwrap(),
            ),
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
            capability_registry: None,
            outcome_tracker: None,
            complexity_analyzer: None,
            container_metrics: None,
            negotiation_protocol: None,
            self_improvement_loop: None,
            self_improvement_interval_turns: 0,
            constitution: None,
            identity_store: None,
            idempotency_store: None,
            mbti_drift_detector: None,
            tool_registry: None,
            pipeline_tools: vec![],
            tool_schemas: vec![],
            turn_event_bus: None,
            workspace_loader: None,
        };
        let rt = AgentRuntime::new(config, deps);
        assert_eq!(rt.max_turns, DEFAULT_MAX_TURNS);
    }

    /// Verifies that [`AgentRuntime::start_autonomous_session`] creates a
    /// session with the requested per-session budgets and `Running` status.
    #[tokio::test]
    async fn test_start_autonomous_session() {
        let config = AgentConfig::new("autonomous-agent", "gpt-4");
        let agent_id = config.id.to_string();
        let deps = RuntimeDependencies {
            provider_router: Arc::new(
                ProviderRouter::new(crate::providers::ProviderRouterConfig::default())
                    .await
                    .unwrap(),
            ),
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
            capability_registry: None,
            outcome_tracker: None,
            complexity_analyzer: None,
            container_metrics: None,
            negotiation_protocol: None,
            self_improvement_loop: None,
            self_improvement_interval_turns: 0,
            constitution: None,
            identity_store: None,
            idempotency_store: None,
            mbti_drift_detector: None,
            tool_registry: None,
            pipeline_tools: vec![],
            tool_schemas: vec![],
            turn_event_bus: None,
            workspace_loader: None,
        };
        let rt = AgentRuntime::new(config, deps);

        // Explicit overrides — both should be respected.
        let session = rt
            .start_autonomous_session(&agent_id, Some(7), Some(0.25))
            .await
            .expect("session must start");
        assert_eq!(session.agent_id, agent_id);
        assert_eq!(session.max_turns, 7);
        assert!((session.cost_budget_usd - 0.25).abs() < 1e-9);
        assert_eq!(session.turns_executed, 0);
        assert!(session.cost_accumulated_usd.abs() < 1e-9);
        assert_eq!(session.status, AutonomousSessionStatus::Running);
        assert!(!session.id.is_empty());

        // None overrides — should fall back to the runtime defaults.
        let defaulted = rt
            .start_autonomous_session(&agent_id, None, None)
            .await
            .unwrap();
        assert_eq!(defaulted.max_turns, DEFAULT_MAX_TURNS);
        assert!((defaulted.cost_budget_usd - DEFAULT_COST_BUDGET_USD).abs() < 1e-9);

        // Mismatched agent_id — validation error.
        let err = rt
            .start_autonomous_session("wrong-agent", None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, ClawzError::Validation(_)));

        // config() accessor — must return the same agent id.
        assert_eq!(rt.config().id.to_string(), agent_id);
    }
}
