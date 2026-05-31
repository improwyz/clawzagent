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

pub async fn load_channel(
    state: &AppState,
    channel_id: &str,
) -> Result<ChannelRecord, GatewayError> {
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

        let conversation_id =
            SessionKey::from_channel(platform, &record.id, &msg.from, &agent_id).storage_id();

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
    verify_webhook_signature(record, headers, body)?;

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

/// Outcome of webhook signature verification.
enum WebhookAuth {
    /// No secret configured for this channel — cannot verify (allowed).
    NoSecret,
    /// Signature present and valid.
    Valid,
    /// Signature present but did not match.
    Invalid,
    /// A secret is configured but no recognized signature header was sent.
    MissingSignature,
}

/// Config keys that may hold a webhook signing secret.
const SECRET_KEYS: &[&str] = &["signing_secret", "webhook_secret", "secret", "app_secret"];

/// Headers carrying an HMAC-SHA256 hex signature (optionally `sha256=` prefixed).
const SIGNATURE_HEADERS: &[&str] = &[
    "x-hub-signature-256",
    "x-signature-256",
    "x-webhook-signature",
    "x-signature",
];

/// Extract and (if sealed) decrypt the channel's webhook secret from its config.
fn channel_secret(config: &Value) -> Option<String> {
    for key in SECRET_KEYS {
        if let Some(s) = config.get(*key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                // Secrets may be sealed at rest; fall back to the raw value.
                return Some(crate::secrets::open_secret(s).unwrap_or_else(|| s.to_string()));
            }
        }
    }
    None
}

/// Whether webhook signature failures should be rejected (vs. warn-only).
///
/// Defaults to warn-only so existing integrations without a configured secret
/// keep working; set `CLAWZ_WEBHOOK_ENFORCE=1` to reject unverified webhooks.
fn webhook_enforce() -> bool {
    std::env::var("CLAWZ_WEBHOOK_ENFORCE").as_deref() == Ok("1")
}

/// Classify an inbound webhook's HMAC-SHA256 signature against the channel secret.
fn classify_webhook_auth(config: &Value, headers: &HeaderMap, body: &[u8]) -> WebhookAuth {
    let Some(secret) = channel_secret(config) else {
        return WebhookAuth::NoSecret;
    };
    let signature = SIGNATURE_HEADERS.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().trim_start_matches("sha256=").to_string())
    });
    let Some(signature) = signature else {
        return WebhookAuth::MissingSignature;
    };
    if verify_hmac_sha256_hex(&secret, body, &signature) {
        WebhookAuth::Valid
    } else {
        WebhookAuth::Invalid
    }
}

/// Verify an HMAC-SHA256 hex signature over `body` using `secret`.
fn verify_hmac_sha256_hex(secret: &str, body: &[u8], signature_hex: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    let Some(provided) = decode_hex(signature_hex) else {
        return false;
    };
    if provided.len() != expected.len() {
        return false;
    }
    // Constant-time comparison to avoid leaking the match position via timing.
    let mut diff = 0u8;
    for (a, b) in provided.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Decode a hex string into bytes (returns `None` on malformed input).
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Reject unverified webhooks when enforcement is enabled; otherwise warn.
fn verify_webhook_signature(
    record: &ChannelRecord,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), GatewayError> {
    match classify_webhook_auth(&record.config, headers, body) {
        WebhookAuth::Valid | WebhookAuth::NoSecret => Ok(()),
        WebhookAuth::Invalid | WebhookAuth::MissingSignature => {
            if webhook_enforce() {
                return Err(GatewayError::Unauthorized(
                    "webhook signature verification failed".into(),
                ));
            }
            tracing::warn!(
                channel_id = %record.id,
                "inbound webhook signature unverified (set CLAWZ_WEBHOOK_ENFORCE=1 to reject)"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod webhook_auth_tests {
    use super::*;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn no_secret_is_allowed() {
        let cfg = serde_json::json!({});
        let h = HeaderMap::new();
        assert!(matches!(
            classify_webhook_auth(&cfg, &h, b"x"),
            WebhookAuth::NoSecret
        ));
    }

    #[test]
    fn valid_signature_accepted() {
        let body = b"hello";
        let sig = sign("s3cr3t", body);
        let cfg = serde_json::json!({ "signing_secret": "s3cr3t" });
        let mut h = HeaderMap::new();
        h.insert("x-hub-signature-256", format!("sha256={sig}").parse().unwrap());
        assert!(matches!(
            classify_webhook_auth(&cfg, &h, body),
            WebhookAuth::Valid
        ));
    }

    #[test]
    fn bad_signature_rejected() {
        let cfg = serde_json::json!({ "signing_secret": "s3cr3t" });
        let mut h = HeaderMap::new();
        h.insert("x-hub-signature-256", "sha256=deadbeef".parse().unwrap());
        assert!(matches!(
            classify_webhook_auth(&cfg, &h, b"hello"),
            WebhookAuth::Invalid
        ));
    }

    #[test]
    fn missing_signature_flagged() {
        let cfg = serde_json::json!({ "webhook_secret": "s3cr3t" });
        let h = HeaderMap::new();
        assert!(matches!(
            classify_webhook_auth(&cfg, &h, b"hello"),
            WebhookAuth::MissingSignature
        ));
    }
}
