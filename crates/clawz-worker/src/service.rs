//! Worker platform service — shared runtime used by control API and in-process gateway.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use clawz_core::deployment::DeploymentMode;
use clawz_core::error::{ClawzError, Result};
use clawz_core::session::SessionStore;
use clawz_core::traits::{AgentScheduler, GovernanceEngine, ToolOrchestrator};
use clawz_core::types::agent::AgentConfig;
use clawz_core::types::message::{ChatRequest, Message};
use clawz_core::types::orchestration::{SpawnConfig, ToolType};
use clawz_services::dto::{
    A2aInvokeRequest, A2aInvokeResponse, EvaluateGovernanceRequest, EvaluateGovernanceResponse,
    ExecuteToolRequest, ExecuteToolResponse, FanOutRequest, FanOutResponse, OrchestrateRequest,
    OrchestrateResponse, ProviderHealthRequest, ProviderHealthResponse, RunTurnRequest,
    RunTurnResponse, SessionSummary, TestChannelRequest, TestChannelResponse,
};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::cron::store::FileJobStore;
use crate::cron::{CreateCronJobRequest, CronRunResult};
use crate::governance::engine::{ClawzGovernanceEngine, GovernanceEngineConfig};
use crate::learning::LearningStack;
use crate::memory::create_memory_backend;
use crate::memory::open_session_store;
use crate::memory::transcript_search::TranscriptHit;
use crate::memory::user_profile::UserProfile;
use crate::orchestration::factory::{
    create_scheduler, create_tool_orchestrator, env_docker_network,
};
use crate::providers::CostTracker;
use crate::providers::router::{ProviderRouter, ProviderRouterConfig, ReliabilityConfig};
use crate::runtime::agent::{AgentRuntime, RuntimeDependencies};
use crate::runtime::fan_out::{AggregationStrategy, FanOut, FanOutConfig};
use crate::runtime::team::{Task, Team, TeamRole};
use crate::runtime::turn_coordinator::{RoomRuntimeProvider, TurnCoordinator};
use crate::runtime::turn_events::TurnEventBus;
use crate::tools::registry::ToolRegistry;
use crate::tools::tool_trait::{ToolConfig, ToolContext};
use std::time::Duration;

/// Shared worker-side service holding runtimes, governance, and tools.
pub struct WorkerService {
    governance: Arc<ClawzGovernanceEngine>,
    tools: Arc<ToolRegistry>,
    provider_router: Arc<ProviderRouter>,
    runtimes: RwLock<HashMap<String, Arc<AgentRuntime>>>,
    turn_coordinator: TurnCoordinator,
    agent_scheduler: Option<Arc<dyn AgentScheduler>>,
    tool_orchestrator: Option<Arc<dyn ToolOrchestrator>>,
    turn_event_bus: Arc<TurnEventBus>,
    session_store: Arc<dyn SessionStore>,
    cron_store: Arc<FileJobStore>,
    learning: Arc<LearningStack>,
}

fn isolated_tool_type(tool_name: &str) -> Option<ToolType> {
    match tool_name {
        "browser" | "browser_navigate" | "web_browser" => Some(ToolType::Browser),
        "sandbox" | "bash" | "shell" | "code_sandbox" => Some(ToolType::Sandbox),
        "mcp" | "mcp_bridge" | "mcp_call" => Some(ToolType::McpBridge),
        _ => None,
    }
}

impl WorkerService {
    pub async fn new() -> Result<Self> {
        Self::new_with_approval(Arc::new(
            crate::governance::approval::ApprovalWorkflow::new(),
        ))
        .await
    }

    pub async fn new_with_approval(
        approval_workflow: Arc<crate::governance::approval::ApprovalWorkflow>,
    ) -> Result<Self> {
        let provider_router = Arc::new(build_provider_router().await?);
        let governance = Arc::new(ClawzGovernanceEngine::new_with_approval(
            GovernanceEngineConfig::default(),
            approval_workflow.clone(),
        ));
        let tools = Arc::new(ToolRegistry::new());
        tools.register_builtins().await;

        let mode = DeploymentMode::from_env();
        let agent_scheduler = match mode {
            DeploymentMode::Standalone => None,
            DeploymentMode::Micro | DeploymentMode::Elastic => create_scheduler(mode).ok(),
        };
        let tool_orchestrator = create_tool_orchestrator(mode).ok();

        let cron_store = Arc::new(FileJobStore::open_default().await?);
        let learning = Arc::new(LearningStack::from_env(approval_workflow).await?);
        crate::memory::rollup::spawn_hourly_rollup_task();
        Ok(Self {
            governance,
            tools,
            provider_router,
            runtimes: RwLock::new(HashMap::new()),
            turn_coordinator: TurnCoordinator::new(),
            agent_scheduler,
            tool_orchestrator,
            turn_event_bus: Arc::new(TurnEventBus::default()),
            session_store: open_session_store().await?,
            cron_store,
            learning,
        })
    }

    pub fn cron_store(&self) -> Arc<FileJobStore> {
        self.cron_store.clone()
    }

    pub fn turn_event_bus(&self) -> Arc<TurnEventBus> {
        self.turn_event_bus.clone()
    }

    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.session_store.clone()
    }

    pub fn learning(&self) -> Arc<LearningStack> {
        self.learning.clone()
    }

    /// Cross-session FTS search over indexed session transcripts.
    pub async fn search_transcripts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TranscriptHit>> {
        self.learning.transcript_search().search(query, limit).await
    }

    /// Load the persisted profile for a tenant (or default empty profile).
    pub async fn user_profile(&self, tenant_id: &str) -> Result<UserProfile> {
        self.learning.user_profiles().load(tenant_id).await
    }

    /// Persist an updated tenant profile.
    pub async fn save_user_profile(&self, profile: &UserProfile) -> Result<()> {
        self.learning.user_profiles().save(profile).await
    }

    /// Trim transcript to the last N messages (dashboard / API compact action).
    pub async fn compact_session(
        &self,
        session_id: &str,
        keep_last: usize,
    ) -> Result<(usize, clawz_core::session::SessionUsage)> {
        let removed = self.session_store.compact(session_id, keep_last).await?;
        let usage = self.session_store.usage(session_id).await?;
        Ok((removed, usage))
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let ids = self.session_store.list_sessions().await?;
        let mut out = Vec::with_capacity(ids.len());
        for session_id in ids {
            let usage = self.session_store.usage(&session_id).await?;
            out.push(SessionSummary {
                session_id,
                message_count: usage.message_count,
                estimated_tokens: usage.estimated_tokens,
            });
        }
        Ok(out)
    }

    async fn build_runtime_deps(
        &self,
        memory: Arc<dyn clawz_core::traits::MemoryBackend>,
        restrict_tools: bool,
        disabled_tools: &[String],
    ) -> RuntimeDependencies {
        let mut deps = RuntimeDependencies::new(
            self.provider_router.clone(),
            memory,
            self.governance.clone(),
            Arc::new(CostTracker::new()),
        )
        .with_turn_event_bus(self.turn_event_bus.clone())
        .with_workspace_loader(Arc::new(crate::workspace::WorkspaceLoader::default_home()));

        if !restrict_tools {
            let names = self.tools.names().await;
            let mut pipeline_tools = Vec::new();
            for name in names {
                if disabled_tools.iter().any(|d| d == &name) {
                    continue;
                }
                if let Some(tool) = self.tools.get(&name).await {
                    pipeline_tools.push(crate::tools::tool_trait::bridge_to_core(tool));
                }
            }
            let schemas: Vec<_> = self
                .tools
                .list()
                .await
                .into_iter()
                .filter(|s| !disabled_tools.iter().any(|d| d == &s.name))
                .collect();
            deps = deps.with_tooling(self.tools.clone(), pipeline_tools, schemas);

            if self.learning.enabled() {
                deps = deps
                    .with_outcome_tracker(self.learning.outcome_tracker())
                    .with_identity_store(self.learning.identity_store())
                    .with_skill_repository(self.learning.skill_repository())
                    .with_proposal_gatekeeper(self.learning.proposal_gatekeeper())
                    .with_audit_logger(self.learning.audit_logger())
                    .with_self_improvement_loop(self.learning.self_improvement_loop())
                    .with_self_improvement_interval(self.learning.interval_turns());
            }
        }

        deps
    }

    pub fn agent_scheduler(&self) -> Option<Arc<dyn AgentScheduler>> {
        self.agent_scheduler.clone()
    }

    pub fn approval_workflow(&self) -> Arc<crate::governance::approval::ApprovalWorkflow> {
        Arc::clone(&self.governance.approval_workflow)
    }

    pub fn governance(&self) -> Arc<ClawzGovernanceEngine> {
        self.governance.clone()
    }

    pub fn tools(&self) -> Arc<ToolRegistry> {
        self.tools.clone()
    }

    pub fn provider_router(&self) -> Arc<ProviderRouter> {
        self.provider_router.clone()
    }

    pub(crate) async fn runtime_for(
        &self,
        agent_id: &str,
        model: Option<&str>,
        system_prompt: Option<&str>,
        restrict_tools: bool,
        disabled_tools: &[String],
    ) -> Result<Arc<AgentRuntime>> {
        if !restrict_tools {
            if let Some(rt) = self.runtimes.read().await.get(agent_id).cloned() {
                if model.is_none() && system_prompt.is_none() {
                    return Ok(rt);
                }
            }
        }

        let model = model.unwrap_or("claude-sonnet-4-5");
        let id = Uuid::parse_str(agent_id).unwrap_or_else(|_| Uuid::new_v4());
        let name = agent_id;
        let mut config = AgentConfig::new(name, model);
        config.id = id;
        if let Some(prompt) = system_prompt {
            config = config.with_system_prompt(prompt);
        }

        let memory: Arc<dyn clawz_core::traits::MemoryBackend> = create_memory_backend().await;

        let deps = self
            .build_runtime_deps(memory, restrict_tools, disabled_tools)
            .await;

        let runtime = Arc::new(AgentRuntime::new(config, deps));
        if !restrict_tools {
            self.runtimes
                .write()
                .await
                .insert(agent_id.to_string(), runtime.clone());
        }
        Ok(runtime)
    }

    pub async fn list_cron_jobs(&self) -> Result<Vec<crate::cron::CronJob>> {
        self.cron_store.list().await
    }

    pub async fn create_cron_job(&self, req: CreateCronJobRequest) -> Result<crate::cron::CronJob> {
        self.cron_store.create(req).await
    }

    pub async fn delete_cron_job(&self, id: &str) -> Result<()> {
        self.cron_store.delete(id).await
    }

    pub async fn execute_cron_job(&self, job_id: &str) -> Result<CronRunResult> {
        let job = self.cron_store.get(job_id).await?;
        let conversation_id = format!("cron-{}", job.id);

        let partial_tools = !job.disabled_toolsets.is_empty();
        let turn = crate::runtime::session_run::execute_agent_turn(
            self,
            &job.agent_id,
            RunTurnRequest {
                message: job.prompt.clone(),
                conversation_id: Some(conversation_id.clone()),
                cron_mode: !partial_tools,
                disabled_tools: job.disabled_toolsets.clone(),
                ..Default::default()
            },
        )
        .await?;

        let mut delivered = false;
        if let Some(delivery) = &job.delivery {
            self.send_channel_message(clawz_services::dto::ChannelSendRequest {
                channel_type: delivery.channel_type.clone(),
                config: delivery.config.clone(),
                content: turn.content.clone(),
                metadata: json!({ "to": delivery.to }),
                agent_id: Some(job.agent_id.clone()),
            })
            .await?;
            delivered = true;
        }

        self.cron_store.mark_run(job_id).await?;

        Ok(CronRunResult {
            job_id: job.id,
            agent_id: job.agent_id,
            conversation_id,
            content: turn.content,
            delivered,
        })
    }

    pub async fn ingest_memory(
        &self,
        agent_id: &str,
        chunks: Vec<crate::background::MemoryIngestChunk>,
    ) -> Result<usize> {
        let _ = self;
        crate::background::ingest_memory_chunks(agent_id, chunks).await
    }

    pub async fn run_subconscious_tick(
        &self,
        agent_id: Option<&str>,
    ) -> Result<clawz_services::dto::SubconsciousTickResponse> {
        let (conversation_id, content, chunks_reviewed) =
            crate::background::subconscious::run_subconscious_tick(self, agent_id).await?;
        Ok(clawz_services::dto::SubconsciousTickResponse {
            agent_id: agent_id.map(str::to_string).unwrap_or_else(|| {
                std::env::var("CLAWZ_SUBCONSCIOUS_AGENT_ID").unwrap_or_else(|_| "default".into())
            }),
            conversation_id,
            content,
            chunks_reviewed,
        })
    }

    pub async fn run_turn(&self, agent_id: &str, req: RunTurnRequest) -> Result<RunTurnResponse> {
        if req.room_id.is_some() {
            return self.turn_coordinator.run_turn(agent_id, req, self).await;
        }

        crate::runtime::session_run::execute_agent_turn(self, agent_id, req).await
    }

    pub async fn execute_tool(&self, req: ExecuteToolRequest) -> Result<ExecuteToolResponse> {
        let start = Instant::now();
        let ctx = ToolContext {
            agent_id: req.agent_id.clone(),
            conversation_id: Uuid::new_v4().to_string(),
            user_id: None,
            config: ToolConfig::default(),
        };

        let tool_handle = if let (Some(orch), Some(tool_type)) = (
            self.tool_orchestrator.as_ref(),
            isolated_tool_type(&req.tool_name),
        ) {
            let config = SpawnConfig {
                memory_mb: 256,
                cpu_millicores: 500,
                image: String::new(),
                env: vec![],
                labels: vec![("owner-agent-id".to_string(), req.agent_id.clone())],
                network: env_docker_network(),
            };
            Some(orch.spawn_tool(tool_type, config).await?)
        } else {
            None
        };

        let result = self.tools.execute(&req.tool_name, &ctx, req.args).await?;

        if let (Some(orch), Some(handle)) = (self.tool_orchestrator.as_ref(), tool_handle.as_ref())
        {
            let _ = orch.reap_tool(handle).await;
        }

        Ok(ExecuteToolResponse {
            tool_name: req.tool_name,
            success: !result.is_error,
            output: json!({ "text": result.output }),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    pub async fn evaluate_governance(
        &self,
        req: EvaluateGovernanceRequest,
    ) -> Result<EvaluateGovernanceResponse> {
        let mut context = req.context;
        if let Some(room_id) = context.get("room_id").and_then(|v| v.as_str()) {
            if !room_id.is_empty() {
                context["room_id"] = json!(room_id);
            }
        }

        let gov = self
            .governance
            .evaluate(&req.agent_id, &req.action, &context)
            .await?;

        let result_label = if gov.allowed {
            "allowed"
        } else if gov.required_approvals > 0 {
            "pending_review"
        } else {
            "blocked"
        };

        let violations: Vec<Value> = gov
            .violations
            .iter()
            .map(|v| json!({ "message": v }))
            .collect();

        Ok(EvaluateGovernanceResponse {
            allowed: gov.allowed,
            result: result_label.to_string(),
            violations,
            detail: json!({
                "trust_score": gov.trust_score,
                "required_approvals": gov.required_approvals,
                "tier": format!("{:?}", gov.tier),
            }),
        })
    }

    pub async fn test_provider(
        &self,
        req: ProviderHealthRequest,
    ) -> Result<ProviderHealthResponse> {
        let start = Instant::now();
        let probe = self
            .provider_router
            .probe_provider(&req.provider_id, req.endpoint, req.api_key)
            .await;

        let (ok, message) = match probe {
            Ok(_) => (true, format!("provider '{}' responded", req.provider_id)),
            Err(e) => (false, e.to_string()),
        };

        Ok(ProviderHealthResponse {
            ok,
            latency_ms: start.elapsed().as_millis() as u64,
            message,
        })
    }

    pub async fn fan_out(&self, req: FanOutRequest) -> Result<FanOutResponse> {
        let models = if req.models.is_empty() {
            vec!["claude-sonnet-4-5".to_string()]
        } else {
            req.models.clone()
        };

        let strategy = match req.strategy.as_deref() {
            Some("concatenate") => AggregationStrategy::Concatenate,
            Some("longest") => AggregationStrategy::Longest,
            _ => AggregationStrategy::First,
        };

        let config = FanOutConfig {
            models: models.clone(),
            max_concurrency: models.len().max(1),
            timeout: Duration::from_secs(60),
            strategy,
        };

        let fan = FanOut::new(self.provider_router.clone(), config);
        let base = ChatRequest::new(
            models
                .first()
                .cloned()
                .unwrap_or_else(|| "claude-sonnet-4-5".into()),
            vec![Message::user(req.prompt)],
        );
        let result = fan.execute(base).await;
        let content = result
            .aggregated
            .and_then(|m| m.content.as_text().map(|s| s.to_string()))
            .ok_or_else(|| ClawzError::Provider("all fan-out models failed".into()))?;

        Ok(FanOutResponse {
            fanout_id: Uuid::new_v4().to_string(),
            content,
            models_used: models,
        })
    }

    pub async fn orchestrate(&self, req: OrchestrateRequest) -> Result<OrchestrateResponse> {
        let room_lock_held = req.room_id.is_some();
        if let Some(room_id) = &req.room_id {
            self.acquire_room_orchestration_lock(room_id).await;
        }

        let team = if let Some(room_id) = &req.room_id {
            Team::new(room_id, req.leader_agent_id.clone())
        } else {
            Team::new("orchestration", req.leader_agent_id.clone())
        };

        for member in &req.member_agent_ids {
            team.add_member(member.clone(), TeamRole::Worker).await;
        }

        let task = Task::new(req.task.clone(), 1);
        let task_id = task.id.clone();
        team.enqueue_task(task).await;

        let orchestration_id = if let Some(room_id) = &req.room_id {
            format!("room:{room_id}:{}", team.id())
        } else {
            team.id().to_string()
        };

        let assignment = team.assign_next().await;

        let (status, assignments) = match assignment {
            Some((tid, agent_id)) => {
                let turn_req = RunTurnRequest {
                    message: req.task,
                    room_id: req.room_id.clone(),
                    conversation_id: req.room_id.clone(),
                    room_snapshot: None,
                    orchestration_run_id: Some(orchestration_id.clone()),
                    room_lock_held,
                    ..Default::default()
                };

                match self.run_turn(&agent_id, turn_req).await {
                    Ok(turn) => (
                        "completed".to_string(),
                        vec![json!({
                            "task_id": tid,
                            "assigned_to": agent_id,
                            "room_id": req.room_id,
                            "trigger_message_id": req.trigger_message_id,
                            "content": turn.content,
                            "role": turn.role,
                            "conversation_id": turn.conversation_id,
                            "message_id": turn.message_id,
                            "delegation_events": turn.delegation_events,
                        })],
                    ),
                    Err(e) => (
                        "running".to_string(),
                        vec![json!({
                            "task_id": tid,
                            "assigned_to": agent_id,
                            "room_id": req.room_id,
                            "trigger_message_id": req.trigger_message_id,
                            "error": e.to_string(),
                        })],
                    ),
                }
            }
            None => (
                "queued".to_string(),
                vec![json!({
                    "task_id": task_id,
                    "assigned_to": null,
                    "room_id": req.room_id,
                    "trigger_message_id": req.trigger_message_id,
                    "note": "queued, no available member",
                })],
            ),
        };

        if room_lock_held {
            if let Some(room_id) = &req.room_id {
                self.release_room_orchestration_lock(room_id).await;
            }
        }

        Ok(OrchestrateResponse {
            orchestration_id,
            status,
            assignments,
        })
    }

    async fn acquire_room_orchestration_lock(&self, room_id: &str) {
        self.turn_coordinator.acquire_room_lock(room_id).await;
    }

    async fn release_room_orchestration_lock(&self, room_id: &str) {
        self.turn_coordinator.release_room_lock(room_id).await;
    }

    fn channel_context(
        platform: &str,
        config_value: serde_json::Value,
        agent_id: String,
    ) -> clawz_core::traits::ChannelContext {
        use clawz_core::traits::ChannelContext;
        use clawz_core::types::channel::ChannelConfig;

        let channel_id = Uuid::new_v4();
        let webhook_url = config_value
            .get("webhook_url")
            .or_else(|| config_value.get("url"))
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let mut config = ChannelConfig::new(platform, config_value);
        config.id = channel_id;
        if let Some(url) = webhook_url {
            config = config.with_webhook(url);
        }
        ChannelContext::new(config, agent_id)
    }

    pub async fn test_channel(&self, req: TestChannelRequest) -> Result<TestChannelResponse> {
        use clawz_core::traits::ChannelPlugin;
        use clawz_core::types::channel::OutgoingMessage;

        let platform = req.channel_type.to_lowercase();
        let agent_id = req.agent_id.unwrap_or_else(|| "system".to_string());
        let ctx = Self::channel_context(&platform, req.config.clone(), agent_id);

        if platform == "google_voice" && req.config.get("account_sid").is_none() {
            return Ok(TestChannelResponse {
                success: true,
                message: "Google Voice bridge configured (inbound webhook only; add Twilio credentials for outbound SMS)".into(),
            });
        }

        let mut test_msg = OutgoingMessage::new(ctx.config.id, "ClawZ channel connectivity test");
        if let Some(to) = req
            .config
            .get("default_to")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            test_msg
                .metadata
                .insert("to".into(), serde_json::Value::String(to.to_string()));
        }

        let send_result = if let Some(plugin) =
            crate::channels::resolve::plugin_for_platform(&platform)
        {
            plugin.send(&ctx, test_msg).await
        } else if ctx.config.webhook_url.is_some() {
            crate::channels::native::webhook::WebhookChannel::new()
                .send(&ctx, test_msg)
                .await
        } else {
            return Ok(TestChannelResponse {
                success: true,
                message: format!(
                    "validated configuration for '{platform}' (dry-run; set default_to or webhook_url to send)"
                ),
            });
        };

        match send_result {
            Ok(()) => Ok(TestChannelResponse {
                success: true,
                message: format!("test message sent via {platform}"),
            }),
            Err(e) => Ok(TestChannelResponse {
                success: false,
                message: e.to_string(),
            }),
        }
    }

    pub async fn process_channel_webhook(
        &self,
        req: clawz_services::dto::ChannelWebhookRequest,
    ) -> Result<clawz_services::dto::ChannelWebhookResponse> {
        use clawz_services::dto::{ChannelWebhookMessage, ChannelWebhookResponse};
        use http::HeaderMap;

        let platform = req.channel_type.to_lowercase();
        let _agent_id = req.agent_id.unwrap_or_else(|| "system".to_string());
        let body = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            req.body_base64.as_bytes(),
        )
        .map_err(|e| clawz_core::error::ClawzError::Serialization(format!("body_base64: {e}")))?;

        let plugin = crate::channels::resolve::plugin_for_platform(&platform).ok_or_else(|| {
            clawz_core::error::ClawzError::Config(format!("unknown channel platform: {platform}"))
        })?;

        let mut headers = HeaderMap::new();
        for (k, v) in req.headers {
            if let (Ok(name), Ok(val)) = (
                http::HeaderName::from_bytes(k.as_bytes()),
                http::HeaderValue::from_str(&v),
            ) {
                headers.insert(name, val);
            }
        }

        let incoming = plugin.webhook(&body, &headers).await?;
        let messages = incoming
            .into_iter()
            .map(|m| ChannelWebhookMessage {
                from: m.sender_id,
                content: m.content,
                metadata: serde_json::Value::Object(m.metadata),
            })
            .collect();

        Ok(ChannelWebhookResponse { messages })
    }

    pub async fn poll_channel(
        &self,
        req: clawz_services::dto::ChannelPollRequest,
    ) -> Result<clawz_services::dto::ChannelPollResponse> {
        use clawz_services::dto::{ChannelPollResponse, ChannelWebhookMessage};

        let platform = req.channel_type.to_lowercase();
        let agent_id = req.agent_id.unwrap_or_else(|| "system".to_string());
        let ctx = Self::channel_context(&platform, req.config, agent_id);

        let plugin = crate::channels::resolve::plugin_for_platform(&platform).ok_or_else(|| {
            clawz_core::error::ClawzError::Config(format!("unknown channel platform: {platform}"))
        })?;

        let incoming = plugin.receive(&ctx).await?;
        let messages = incoming
            .into_iter()
            .map(|m| ChannelWebhookMessage {
                from: m.sender_id,
                content: m.content,
                metadata: serde_json::Value::Object(m.metadata),
            })
            .collect();

        Ok(ChannelPollResponse { messages })
    }

    pub async fn send_channel_message(
        &self,
        req: clawz_services::dto::ChannelSendRequest,
    ) -> Result<clawz_services::dto::ChannelSendResponse> {
        use clawz_core::types::channel::OutgoingMessage;
        use clawz_services::dto::ChannelSendResponse;

        let platform = req.channel_type.to_lowercase();
        let agent_id = req.agent_id.unwrap_or_else(|| "system".to_string());
        let ctx = Self::channel_context(&platform, req.config, agent_id);

        let plugin = crate::channels::resolve::plugin_for_platform(&platform).ok_or_else(|| {
            clawz_core::error::ClawzError::Config(format!("unknown channel platform: {platform}"))
        })?;

        let mut msg = OutgoingMessage::new(ctx.config.id, req.content);
        if let Some(obj) = req.metadata.as_object() {
            for (k, v) in obj {
                msg.metadata.insert(k.clone(), v.clone());
            }
        }

        plugin.send(&ctx, msg).await?;
        Ok(ChannelSendResponse { success: true })
    }

    pub async fn a2a_invoke(&self, req: A2aInvokeRequest) -> Result<A2aInvokeResponse> {
        let turn = self
            .run_turn(
                &req.to_agent_id,
                RunTurnRequest {
                    message: format!("[from {}] {}", req.from_agent_id, req.message),
                    ..Default::default()
                },
            )
            .await?;

        Ok(A2aInvokeResponse {
            status: "completed".to_string(),
            result: json!({
                "content": turn.content,
                "role": turn.role,
            }),
        })
    }
}

#[async_trait::async_trait]
impl RoomRuntimeProvider for WorkerService {
    async fn runtime_for_turn(
        &self,
        agent_id: &str,
        req: &RunTurnRequest,
    ) -> Result<Arc<AgentRuntime>> {
        self.runtime_for(
            agent_id,
            req.model.as_deref(),
            req.system_prompt.as_deref(),
            req.cron_mode || req.background_mode,
            &req.disabled_tools,
        )
        .await
    }

    async fn prepare_room_turn(
        &self,
        agent_id: &str,
        req: &RunTurnRequest,
        runtime: &Arc<AgentRuntime>,
    ) -> Result<()> {
        let room_id = req
            .room_id
            .as_deref()
            .ok_or_else(|| ClawzError::Validation("room_id required for room turn".into()))?;

        if let Some(cap) =
            crate::runtime::turn_coordinator::room_budget_cap(req.room_snapshot.as_ref())
        {
            let spent = runtime.cost_tracker().room_total(room_id).await;
            if spent >= cap {
                return Err(ClawzError::Provider(format!(
                    "room budget exceeded: ${spent:.4} >= ${cap:.2}"
                )));
            }
        }

        let mut gov_context = json!({
            "room_id": room_id,
            "sender_user_id": req.sender_user_id,
            "orchestration_run_id": req.orchestration_run_id,
            "visibility": req.visibility,
            "message_preview": req.message.chars().take(200).collect::<String>(),
        });
        if let Some(snapshot) = &req.room_snapshot {
            gov_context["room_snapshot"] = snapshot.clone();
        }

        let gov = self
            .governance
            .evaluate(agent_id, "room_turn", &gov_context)
            .await?;
        if !gov.allowed {
            return Err(ClawzError::Governance(format!(
                "room turn denied: {:?}",
                gov.violations
            )));
        }

        runtime
            .cost_tracker()
            .set_attribution(crate::providers::cost::CostAttribution {
                room_id: Some(room_id.to_string()),
                triggered_by_user_id: req.sender_user_id.clone(),
                orchestration_run_id: req.orchestration_run_id.clone(),
            })
            .await;

        Ok(())
    }
}

async fn build_provider_router() -> Result<ProviderRouter> {
    let mut providers = crate::providers::config::config_from_env().providers;

    if providers.is_empty()
        || std::env::var("CLAWZ_STUB_PROVIDER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    {
        providers.insert(
            "stub".to_string(),
            crate::providers::router::ProviderConfig {
                endpoint: "local://stub".to_string(),
                api_key: String::new(),
                models: vec![],
                auth_type: crate::providers::router::AuthType::None,
                ..Default::default()
            },
        );
    }

    let reliability = ReliabilityConfig {
        max_retries: 3,
        base_delay_ms: 500,
        max_delay_ms: 30_000,
        exponential_base: 2.0,
        jitter: 0.1,
        circuit_breaker_threshold: 5,
        circuit_breaker_timeout_secs: 30,
    };

    let config = ProviderRouterConfig {
        providers,
        reliability,
        budget: None,
    };

    ProviderRouter::new(config).await
}
