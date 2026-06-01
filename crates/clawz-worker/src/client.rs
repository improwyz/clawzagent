//! In-process execution client for standalone mode (gateway embeds worker).

use std::sync::Arc;

use async_trait::async_trait;
use clawz_services::dto::{
    A2aInvokeRequest, A2aInvokeResponse, ChannelPollRequest, ChannelPollResponse,
    ChannelSendRequest, ChannelSendResponse, ChannelWebhookRequest, ChannelWebhookResponse,
    CompactSessionRequest, CompactSessionResponse, CreateCronJobRequest, CronJobDto,
    CronRunResultDto, EvaluateGovernanceRequest, EvaluateGovernanceResponse, ExecuteToolRequest,
    ExecuteToolResponse, FanOutRequest, FanOutResponse, MemoryIngestRequest, MemoryIngestResponse,
    OrchestrateRequest, OrchestrateResponse, ProviderHealthRequest, ProviderHealthResponse,
    RunTurnRequest, RunTurnResponse, SessionSummary, SubconsciousTickRequest,
    SubconsciousTickResponse, TestChannelRequest, TestChannelResponse,
};
use clawz_services::execution::{ExecutionClient, ExecutionError, ExecutionResult};

use crate::service::WorkerService;

/// Delegates to a local [`WorkerService`] without HTTP.
pub struct InProcessExecutionClient {
    service: Arc<WorkerService>,
}

impl InProcessExecutionClient {
    pub fn new(service: Arc<WorkerService>) -> Self {
        Self { service }
    }

    fn map_err(e: clawz_core::error::ClawzError) -> ExecutionError {
        ExecutionError::Transport(e.to_string())
    }
}

#[async_trait]
impl ExecutionClient for InProcessExecutionClient {
    async fn run_turn(
        &self,
        agent_id: &str,
        req: RunTurnRequest,
    ) -> ExecutionResult<RunTurnResponse> {
        self.service
            .run_turn(agent_id, req)
            .await
            .map_err(Self::map_err)
    }

    async fn list_sessions(&self) -> ExecutionResult<Vec<SessionSummary>> {
        self.service.list_sessions().await.map_err(Self::map_err)
    }

    async fn compact_session(
        &self,
        session_id: &str,
        req: CompactSessionRequest,
    ) -> ExecutionResult<CompactSessionResponse> {
        let keep = req
            .keep_last
            .unwrap_or(crate::runtime::session_commands::DEFAULT_COMPACT_KEEP);
        let (removed, usage) = self
            .service
            .compact_session(session_id, keep)
            .await
            .map_err(Self::map_err)?;
        Ok(CompactSessionResponse {
            session_id: session_id.to_string(),
            removed,
            message_count: usage.message_count,
            estimated_tokens: usage.estimated_tokens,
        })
    }

    async fn execute_tool(&self, req: ExecuteToolRequest) -> ExecutionResult<ExecuteToolResponse> {
        self.service.execute_tool(req).await.map_err(Self::map_err)
    }

    async fn evaluate_governance(
        &self,
        req: EvaluateGovernanceRequest,
    ) -> ExecutionResult<EvaluateGovernanceResponse> {
        self.service
            .evaluate_governance(req)
            .await
            .map_err(Self::map_err)
    }

    async fn test_provider(
        &self,
        req: ProviderHealthRequest,
    ) -> ExecutionResult<ProviderHealthResponse> {
        self.service.test_provider(req).await.map_err(Self::map_err)
    }

    async fn fan_out(&self, req: FanOutRequest) -> ExecutionResult<FanOutResponse> {
        self.service.fan_out(req).await.map_err(Self::map_err)
    }

    async fn orchestrate(&self, req: OrchestrateRequest) -> ExecutionResult<OrchestrateResponse> {
        self.service.orchestrate(req).await.map_err(Self::map_err)
    }

    async fn a2a_invoke(&self, req: A2aInvokeRequest) -> ExecutionResult<A2aInvokeResponse> {
        self.service.a2a_invoke(req).await.map_err(Self::map_err)
    }

    async fn test_channel(&self, req: TestChannelRequest) -> ExecutionResult<TestChannelResponse> {
        self.service.test_channel(req).await.map_err(Self::map_err)
    }

    async fn process_channel_webhook(
        &self,
        req: ChannelWebhookRequest,
    ) -> ExecutionResult<ChannelWebhookResponse> {
        self.service
            .process_channel_webhook(req)
            .await
            .map_err(Self::map_err)
    }

    async fn send_channel_message(
        &self,
        req: ChannelSendRequest,
    ) -> ExecutionResult<ChannelSendResponse> {
        self.service
            .send_channel_message(req)
            .await
            .map_err(Self::map_err)
    }

    async fn poll_channel(&self, req: ChannelPollRequest) -> ExecutionResult<ChannelPollResponse> {
        self.service.poll_channel(req).await.map_err(Self::map_err)
    }

    async fn list_cron_jobs(&self) -> ExecutionResult<Vec<CronJobDto>> {
        let jobs = self.service.list_cron_jobs().await.map_err(Self::map_err)?;
        Ok(jobs
            .into_iter()
            .map(crate::cron::convert::job_to_dto)
            .collect())
    }

    async fn create_cron_job(&self, req: CreateCronJobRequest) -> ExecutionResult<CronJobDto> {
        let job = self
            .service
            .create_cron_job(crate::cron::convert::create_from_dto(req))
            .await
            .map_err(Self::map_err)?;
        Ok(crate::cron::convert::job_to_dto(job))
    }

    async fn delete_cron_job(&self, job_id: &str) -> ExecutionResult<()> {
        self.service
            .delete_cron_job(job_id)
            .await
            .map_err(Self::map_err)
    }

    async fn run_cron_job(&self, job_id: &str) -> ExecutionResult<CronRunResultDto> {
        let result = self
            .service
            .execute_cron_job(job_id)
            .await
            .map_err(Self::map_err)?;
        Ok(crate::cron::convert::run_to_dto(result))
    }

    async fn ingest_memory(
        &self,
        req: MemoryIngestRequest,
    ) -> ExecutionResult<MemoryIngestResponse> {
        let chunks = req
            .chunks
            .into_iter()
            .map(|c| crate::background::MemoryIngestChunk {
                key: c.key,
                text: c.text,
                source: c.source,
            })
            .collect();
        let stored = self
            .service
            .ingest_memory(&req.agent_id, chunks)
            .await
            .map_err(Self::map_err)?;
        Ok(MemoryIngestResponse { stored })
    }

    async fn run_subconscious_tick(
        &self,
        req: SubconsciousTickRequest,
    ) -> ExecutionResult<SubconsciousTickResponse> {
        self.service
            .run_subconscious_tick(req.agent_id.as_deref())
            .await
            .map_err(Self::map_err)
    }
}
