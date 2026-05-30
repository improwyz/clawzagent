//! Request/response types for gateway ↔ worker control API.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunTurnRequest {
    pub message: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Multi-participant room identifier (when set, routes via TurnCoordinator).
    #[serde(default)]
    pub room_id: Option<String>,
    /// Human sender within the room (for attribution and routing hints).
    #[serde(default)]
    pub sender_user_id: Option<String>,
    /// Routing override: `leader`, `mention:{id}`, or `pattern:{name}`.
    #[serde(default)]
    pub routing_hint: Option<String>,
    /// Visibility scope for the turn (e.g. room, side-thread).
    #[serde(default)]
    pub visibility: Option<String>,
    /// Participant list and room metadata forwarded from the gateway.
    #[serde(default)]
    pub room_snapshot: Option<Value>,
    /// Active orchestration run for cost and audit attribution.
    #[serde(default)]
    pub orchestration_run_id: Option<String>,
    /// When true, the caller already holds the per-room turn lock.
    #[serde(default)]
    pub room_lock_held: bool,
    /// Cron runs disable interactive tools (shell, browser, etc.).
    #[serde(default)]
    pub cron_mode: bool,
    /// Background / subconscious ticks disable interactive tools.
    #[serde(default)]
    pub background_mode: bool,
    /// Tool names to omit from the pipeline (cron `disabled_toolsets`).
    #[serde(default)]
    pub disabled_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryIngestChunk {
    pub key: String,
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryIngestRequest {
    pub agent_id: String,
    pub chunks: Vec<MemoryIngestChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryIngestResponse {
    pub stored: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubconsciousTickRequest {
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubconsciousTickResponse {
    pub agent_id: String,
    pub conversation_id: String,
    pub content: String,
    pub chunks_reviewed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunTurnResponse {
    pub agent_id: String,
    pub conversation_id: String,
    pub content: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation_events: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub message_count: usize,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteToolRequest {
    pub agent_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteToolResponse {
    pub tool_name: String,
    pub success: bool,
    pub output: Value,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateGovernanceRequest {
    pub agent_id: String,
    pub action: String,
    #[serde(default)]
    pub context: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateGovernanceResponse {
    pub allowed: bool,
    pub result: String,
    pub violations: Vec<Value>,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthRequest {
    pub provider_id: String,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthResponse {
    pub ok: bool,
    pub latency_ms: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanOutRequest {
    pub prompt: String,
    pub models: Vec<String>,
    #[serde(default)]
    pub strategy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanOutResponse {
    pub fanout_id: String,
    pub content: String,
    pub models_used: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrchestrateRequest {
    pub leader_agent_id: String,
    pub task: String,
    pub member_agent_ids: Vec<String>,
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub trigger_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrateResponse {
    pub orchestration_id: String,
    pub status: String,
    pub assignments: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aInvokeRequest {
    pub from_agent_id: String,
    pub to_agent_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aInvokeResponse {
    pub status: String,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestChannelRequest {
    pub channel_type: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestChannelResponse {
    pub success: bool,
    pub message: String,
}

/// Inbound provider webhook forwarded from the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelWebhookRequest {
    pub channel_type: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Raw HTTP body (standard base64).
    pub body_base64: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelWebhookMessage {
    pub from: String,
    pub content: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelWebhookResponse {
    pub messages: Vec<ChannelWebhookMessage>,
}

/// Outbound SMS/voice message via a phone channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSendRequest {
    pub channel_type: String,
    #[serde(default)]
    pub config: Value,
    pub content: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSendResponse {
    pub success: bool,
}

/// Poll a channel adapter for new inbound messages (supervisor loop).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelPollRequest {
    pub channel_type: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelPollResponse {
    pub messages: Vec<ChannelWebhookMessage>,
}

/// Cron job delivery target (channel adapter + recipient).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronDeliveryDto {
    pub channel_type: String,
    #[serde(default)]
    pub config: Value,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobDto {
    pub id: String,
    pub name: String,
    pub cron_expr: String,
    pub prompt: String,
    pub agent_id: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<CronDeliveryDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_toolsets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCronJobRequest {
    pub cron_expr: String,
    pub prompt: String,
    pub agent_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub delivery: Option<CronDeliveryDto>,
    #[serde(default)]
    pub disabled_toolsets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRunResultDto {
    pub job_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub content: String,
    pub delivered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactSessionRequest {
    #[serde(default)]
    pub keep_last: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactSessionResponse {
    pub session_id: String,
    pub removed: usize,
    pub message_count: usize,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousLoopRequest {
    pub agent_id: String,
    pub max_turns: usize,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub seed_message: Option<String>,
}
