//! Shared inbound channel handling — webhooks and supervisor polls → agent runs.

use axum::http::HeaderMap;
use base64::Engine;
use clawz_core::session::SessionKey;
use clawz_services::dto::{ChannelSendRequest, ChannelWebhookRequest, RunTurnRequest};
use serde_json::Value;

use crate::channel_pairing::PairingStore;
use crate::{AppState, ChannelRecord, GatewayError};

pub fn agent_id_from_config(config: &Value) -> Option<String> {
    config
        .get("agent_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub async fn load_channel(state: &AppState, channel_id: &str) -> Result<ChannelRecord, GatewayError> {
    let channels = state.channels.read().await;
    let record = channels
        .iter()
        .find(|c| c.id == channel_id)
        .cloned()
        .ok_or_else(|| GatewayError::not_found("Channel", channel_id))?;
    if !record.enabled {
        return Err(GatewayError::Unprocessable("channel is disabled".into()));
    }
    Ok(record)
}

/// Run agent turns for parsed inbound messages and return `(to, reply)` pairs.
pub async fn dispatch_parsed_messages(
    state: &AppState,
    record: &ChannelRecord,
    platform: &str,
    messages: Vec<clawz_services::dto::ChannelWebhookMessage>,
) -> Result<Vec<(String, String)>, GatewayError> {
    let agent_id = agent_id_from_config(&record.config)
        .ok_or_else(|| GatewayError::Unprocessable("channel config missing agent_id".into()))?;

    let platform_exec = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let pairing = PairingStore::global();
    let mut replies = Vec::new();

    for msg in messages {
        if msg.content.trim().is_empty() {
            continue;
        }
        if !pairing
            .is_allowed(&record.id, &msg.from, &record.config)
            .await
        {
            tracing::info!(
                channel_id = %record.id,
                peer = %msg.from,
                "inbound blocked — peer not paired"
            );
            continue;
        }

        let conversation_id = SessionKey::from_channel(platform, &record.id, &msg.from, &agent_id)
            .storage_id();

        let turn = platform_exec
            .execution
            .run_turn(
                &agent_id,
                RunTurnRequest {
                    message: msg.content.clone(),
                    model: None,
                    system_prompt: None,
                    conversation_id: Some(conversation_id),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        replies.push((msg.from, turn.content));
    }
    Ok(replies)
}

pub async fn process_webhook_body(
    state: &AppState,
    record: &ChannelRecord,
    platform: &str,
    body: &[u8],
    headers: &HeaderMap,
) -> Result<Vec<(String, String)>, GatewayError> {
    let platform_exec = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let mut header_map = std::collections::HashMap::new();
    for (k, v) in headers.iter() {
        if let (Ok(name), Ok(val)) = (k.as_str().parse::<String>(), v.to_str()) {
            header_map.insert(name, val.to_string());
        }
    }

    let parsed = platform_exec
        .execution
        .process_channel_webhook(ChannelWebhookRequest {
            channel_type: platform.to_string(),
            config: record.config.clone(),
            agent_id: agent_id_from_config(&record.config),
            body_base64: base64::engine::general_purpose::STANDARD.encode(body),
            content_type: headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            headers: header_map,
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    dispatch_parsed_messages(state, record, platform, parsed.messages).await
}

pub async fn send_reply(
    state: &AppState,
    record: &ChannelRecord,
    to: &str,
    content: &str,
) -> Result<(), GatewayError> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let metadata = serde_json::json!({ "to": to });
    platform
        .execution
        .send_channel_message(ChannelSendRequest {
            channel_type: record.channel_type.clone(),
            config: record.config.clone(),
            content: content.to_string(),
            metadata,
            agent_id: agent_id_from_config(&record.config),
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(())
}
