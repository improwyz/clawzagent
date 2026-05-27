//! In-process execution client for standalone mode (gateway embeds worker).

use std::sync::Arc;

use async_trait::async_trait;
use clawz_services::dto::{
    A2aInvokeRequest, A2aInvokeResponse, ChannelSendRequest, ChannelSendResponse,
    ChannelWebhookRequest, ChannelWebhookResponse, EvaluateGovernanceRequest,
    EvaluateGovernanceResponse, ExecuteToolRequest, ExecuteToolResponse, FanOutRequest,
    FanOutResponse, OrchestrateRequest, OrchestrateResponse, ProviderHealthRequest,
    ProviderHealthResponse, RunTurnRequest, RunTurnResponse, TestChannelRequest, TestChannelResponse,
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
    async fn run_turn(&self, agent_id: &str, req: RunTurnRequest) -> ExecutionResult<RunTurnResponse> {
        self.service
            .run_turn(agent_id, req)
            .await
            .map_err(Self::map_err)
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
}
