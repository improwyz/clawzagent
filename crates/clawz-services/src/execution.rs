//! Execution client — gateway delegates agent/tool/governance work to the worker.

use async_trait::async_trait;

use crate::dto::{
    A2aInvokeRequest, A2aInvokeResponse, ChannelPollRequest, ChannelPollResponse,
    ChannelSendRequest, ChannelSendResponse, ChannelWebhookRequest, ChannelWebhookResponse,
    CompactSessionRequest, CompactSessionResponse, CreateCronJobRequest, CronJobDto,
    CronRunResultDto, EvaluateGovernanceRequest, EvaluateGovernanceResponse, ExecuteToolRequest,
    ExecuteToolResponse, FanOutRequest, FanOutResponse, MemoryIngestRequest, MemoryIngestResponse,
    OrchestrateRequest, OrchestrateResponse, ProviderHealthRequest, ProviderHealthResponse,
    RunTurnRequest, RunTurnResponse, SessionSummary, SubconsciousTickRequest,
    SubconsciousTickResponse, TestChannelRequest, TestChannelResponse,
};

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("worker error ({status}): {message}")]
    Remote { status: u16, message: String },
    #[error("serialization: {0}")]
    Serialization(String),
}

pub type ExecutionResult<T> = Result<T, ExecutionError>;

/// All runtime operations the gateway delegates to the worker control plane.
#[async_trait]
pub trait ExecutionClient: Send + Sync {
    async fn run_turn(
        &self,
        agent_id: &str,
        req: RunTurnRequest,
    ) -> ExecutionResult<RunTurnResponse>;

    async fn compact_session(
        &self,
        session_id: &str,
        req: CompactSessionRequest,
    ) -> ExecutionResult<CompactSessionResponse>;

    async fn list_sessions(&self) -> ExecutionResult<Vec<SessionSummary>>;

    async fn execute_tool(&self, req: ExecuteToolRequest) -> ExecutionResult<ExecuteToolResponse>;

    async fn evaluate_governance(
        &self,
        req: EvaluateGovernanceRequest,
    ) -> ExecutionResult<EvaluateGovernanceResponse>;

    async fn test_provider(
        &self,
        req: ProviderHealthRequest,
    ) -> ExecutionResult<ProviderHealthResponse>;

    async fn fan_out(&self, req: FanOutRequest) -> ExecutionResult<FanOutResponse>;

    async fn orchestrate(&self, req: OrchestrateRequest) -> ExecutionResult<OrchestrateResponse>;

    async fn a2a_invoke(&self, req: A2aInvokeRequest) -> ExecutionResult<A2aInvokeResponse>;

    async fn test_channel(&self, req: TestChannelRequest) -> ExecutionResult<TestChannelResponse>;

    async fn process_channel_webhook(
        &self,
        req: ChannelWebhookRequest,
    ) -> ExecutionResult<ChannelWebhookResponse>;

    async fn send_channel_message(
        &self,
        req: ChannelSendRequest,
    ) -> ExecutionResult<ChannelSendResponse>;

    async fn poll_channel(&self, req: ChannelPollRequest) -> ExecutionResult<ChannelPollResponse>;

    async fn list_cron_jobs(&self) -> ExecutionResult<Vec<CronJobDto>>;

    async fn create_cron_job(&self, req: CreateCronJobRequest) -> ExecutionResult<CronJobDto>;

    async fn delete_cron_job(&self, job_id: &str) -> ExecutionResult<()>;

    async fn run_cron_job(&self, job_id: &str) -> ExecutionResult<CronRunResultDto>;

    async fn ingest_memory(
        &self,
        req: MemoryIngestRequest,
    ) -> ExecutionResult<MemoryIngestResponse>;

    async fn run_subconscious_tick(
        &self,
        req: SubconsciousTickRequest,
    ) -> ExecutionResult<SubconsciousTickResponse>;
}

/// HTTP client for the worker control API (`WORKER_URL`, default port 50051).
pub struct HttpExecutionClient {
    base_url: String,
    http: reqwest::Client,
}

impl HttpExecutionClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> ExecutionResult<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).json(body);
        if let Some(token) = std::env::var("CLAWZ_WORKER_TOKEN")
            .ok()
            .or_else(|| std::env::var("WORKER_INTERNAL_TOKEN").ok())
            .filter(|t| !t.is_empty())
        {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ExecutionError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ExecutionError::Remote {
                status: status.as_u16(),
                message,
            });
        }

        resp.json::<T>()
            .await
            .map_err(|e| ExecutionError::Serialization(e.to_string()))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> ExecutionResult<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.get(&url);
        if let Some(token) = std::env::var("CLAWZ_WORKER_TOKEN")
            .ok()
            .or_else(|| std::env::var("WORKER_INTERNAL_TOKEN").ok())
            .filter(|t| !t.is_empty())
        {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ExecutionError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ExecutionError::Remote {
                status: status.as_u16(),
                message,
            });
        }
        resp.json::<T>()
            .await
            .map_err(|e| ExecutionError::Serialization(e.to_string()))
    }

    async fn delete_json(&self, path: &str) -> ExecutionResult<()> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.delete(&url);
        if let Some(token) = std::env::var("CLAWZ_WORKER_TOKEN")
            .ok()
            .or_else(|| std::env::var("WORKER_INTERNAL_TOKEN").ok())
            .filter(|t| !t.is_empty())
        {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ExecutionError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(ExecutionError::Remote { status, message });
        }
        Ok(())
    }
}

#[async_trait]
impl ExecutionClient for HttpExecutionClient {
    async fn run_turn(
        &self,
        agent_id: &str,
        req: RunTurnRequest,
    ) -> ExecutionResult<RunTurnResponse> {
        self.post_json(&format!("/v1/agents/{agent_id}/run"), &req)
            .await
    }

    async fn compact_session(
        &self,
        session_id: &str,
        req: CompactSessionRequest,
    ) -> ExecutionResult<CompactSessionResponse> {
        self.post_json(&format!("/v1/sessions/{session_id}/compact"), &req)
            .await
    }

    async fn list_sessions(&self) -> ExecutionResult<Vec<SessionSummary>> {
        self.get_json("/v1/sessions").await
    }

    async fn execute_tool(&self, req: ExecuteToolRequest) -> ExecutionResult<ExecuteToolResponse> {
        self.post_json("/v1/tools/execute", &req).await
    }

    async fn evaluate_governance(
        &self,
        req: EvaluateGovernanceRequest,
    ) -> ExecutionResult<EvaluateGovernanceResponse> {
        self.post_json("/v1/governance/evaluate", &req).await
    }

    async fn test_provider(
        &self,
        req: ProviderHealthRequest,
    ) -> ExecutionResult<ProviderHealthResponse> {
        self.post_json("/v1/providers/health", &req).await
    }

    async fn fan_out(&self, req: FanOutRequest) -> ExecutionResult<FanOutResponse> {
        self.post_json("/v1/fanout", &req).await
    }

    async fn orchestrate(&self, req: OrchestrateRequest) -> ExecutionResult<OrchestrateResponse> {
        self.post_json("/v1/orchestrate", &req).await
    }

    async fn a2a_invoke(&self, req: A2aInvokeRequest) -> ExecutionResult<A2aInvokeResponse> {
        self.post_json("/v1/a2a/invoke", &req).await
    }

    async fn test_channel(&self, req: TestChannelRequest) -> ExecutionResult<TestChannelResponse> {
        self.post_json("/v1/channels/test", &req).await
    }

    async fn process_channel_webhook(
        &self,
        req: ChannelWebhookRequest,
    ) -> ExecutionResult<ChannelWebhookResponse> {
        self.post_json("/v1/channels/webhook", &req).await
    }

    async fn send_channel_message(
        &self,
        req: ChannelSendRequest,
    ) -> ExecutionResult<ChannelSendResponse> {
        self.post_json("/v1/channels/send", &req).await
    }

    async fn poll_channel(&self, req: ChannelPollRequest) -> ExecutionResult<ChannelPollResponse> {
        self.post_json("/v1/channels/poll", &req).await
    }

    async fn list_cron_jobs(&self) -> ExecutionResult<Vec<CronJobDto>> {
        #[derive(serde::Deserialize)]
        struct ListResp {
            data: Vec<CronJobDto>,
        }
        let resp: ListResp = self.get_json("/v1/cron/jobs").await?;
        Ok(resp.data)
    }

    async fn create_cron_job(&self, req: CreateCronJobRequest) -> ExecutionResult<CronJobDto> {
        self.post_json("/v1/cron/jobs", &req).await
    }

    async fn delete_cron_job(&self, job_id: &str) -> ExecutionResult<()> {
        self.delete_json(&format!("/v1/cron/jobs/{job_id}")).await
    }

    async fn run_cron_job(&self, job_id: &str) -> ExecutionResult<CronRunResultDto> {
        self.post_json(
            &format!("/v1/cron/jobs/{job_id}/run"),
            &serde_json::json!({}),
        )
        .await
    }

    async fn ingest_memory(
        &self,
        req: MemoryIngestRequest,
    ) -> ExecutionResult<MemoryIngestResponse> {
        self.post_json("/v1/background/ingest", &req).await
    }

    async fn run_subconscious_tick(
        &self,
        req: SubconsciousTickRequest,
    ) -> ExecutionResult<SubconsciousTickResponse> {
        self.post_json("/v1/background/subconscious", &req).await
    }
}
