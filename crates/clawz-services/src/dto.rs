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
