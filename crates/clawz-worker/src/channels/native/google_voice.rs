//! Google Voice bridge channel for the ClawZ worker.
//!
//! Google Voice does not expose a public REST API for consumer accounts. This channel
//! accepts inbound events from a [documented bridge payload](https://github.com/enterpryz/clawzagent/blob/main/docs/telephony-google-voice-bridge.md)
//! (Apps Script, Zapier, or IFTTT) and can send SMS via an optional linked Twilio
//! fallback when `twilio_*` credentials are present.
//!
//! # Required credentials
//! | key | description |
//! |-----|-------------|
//! | `phone_number` | Agent's Google Voice number (E.164) |
//! | `bridge_secret` | Shared secret for `X-Clawz-Signature` HMAC verification |
//!
//! # Optional (outbound SMS via Twilio)
//! | key | description |
//! |-----|-------------|
//! | `account_sid`, `auth_token` | Twilio account used as SMS send fallback |
//! | `agent_id` | Bound agent UUID (gateway webhooks) |

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
use clawz_core::types::channel::{ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::channels::native::twilio::TwilioChannel;

type HmacSha256 = Hmac<Sha256>;

/// Google Voice bridge channel (webhook-first).
pub struct GoogleVoiceChannel;

impl GoogleVoiceChannel {
    pub fn new() -> Self {
        Self
    }

    /// Verify `X-Clawz-Signature: sha256=<hex>` when a secret is configured.
    pub fn verify_signature(secret: &str, payload: &[u8], header: Option<&str>) -> Result<()> {
        let Some(sig_header) = header else {
            return Err(ClawzError::Auth(
                "Google Voice bridge: missing X-Clawz-Signature".into(),
            ));
        };
        let expected = sig_header
            .strip_prefix("sha256=")
            .ok_or_else(|| ClawzError::Auth("invalid signature format".into()))?;

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| ClawzError::Auth(e.to_string()))?;
        mac.update(payload);
        let computed = hex::encode(mac.finalize().into_bytes());

        if computed != expected {
            return Err(ClawzError::Auth(
                "Google Voice bridge: signature mismatch".into(),
            ));
        }
        Ok(())
    }

    fn parse_bridge_json(channel_id: uuid::Uuid, body: &Value) -> Vec<IncomingMessage> {
        let event = body
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("sms.received");

        if event.contains("call") {
            let from = body
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let status = body
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let text = format!("Google Voice call from {from} ({status})");
            let mut im = IncomingMessage::new(channel_id, from.clone(), from, text);
            im.metadata.insert(
                "google_voice_kind".into(),
                Value::String("call".into()),
            );
            return vec![im];
        }

        let from = body
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let text = body
            .get("body")
            .or_else(|| body.get("text"))
            .or_else(|| body.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if text.is_empty() {
            return Vec::new();
        }

        let mut im = IncomingMessage::new(channel_id, from.clone(), from, text);
        im.metadata.insert(
            "google_voice_kind".into(),
            Value::String("sms".into()),
        );
        if let Some(id) = body.get("message_id").or_else(|| body.get("id")) {
            im.metadata.insert("google_voice_message_id".into(), id.clone());
        }
        vec![im]
    }

    fn has_twilio_fallback(ctx: &ChannelContext) -> bool {
        ctx.config.credentials.get("account_sid").is_some()
            && ctx.config.credentials.get("auth_token").is_some()
    }
}

impl Default for GoogleVoiceChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for GoogleVoiceChannel {
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Google Voice".into(),
            platform: "google_voice".into(),
            version: "1.0.0".into(),
            author: "ClawZ".into(),
        }
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            media: false,
            typing: false,
            reactions: false,
            threads: false,
            voice: true,
            video: false,
            file_upload: false,
        }
    }

    async fn receive(&self, _ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        // Google Voice has no poll API; inbound is webhook/bridge only.
        Ok(Vec::new())
    }

    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        if Self::has_twilio_fallback(ctx) {
            return TwilioChannel::new().send(ctx, msg).await;
        }

        Err(ClawzError::Channel(
            "Google Voice has no public send API; configure Twilio credentials on this channel \
             for outbound SMS, or reply via the bridge".into(),
        ))
    }

    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        // HMAC verification uses credentials and runs in the gateway before this is called.
        let body: Value = serde_json::from_slice(payload).map_err(|e| {
            ClawzError::Serialization(format!("Google Voice bridge JSON: {e}"))
        })?;

        Ok(Self::parse_bridge_json(uuid::Uuid::new_v4(), &body))
    }
}

/// Verify bridge signature when `secret` and header are present (gateway helper).
pub fn verify_google_voice_signature(
    secret: &str,
    payload: &[u8],
    signature_header: Option<&str>,
) -> Result<()> {
    GoogleVoiceChannel::verify_signature(secret, payload, signature_header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verify_google_voice_signature_accepts_valid_hmac() {
        let secret = "test-secret";
        let payload = br#"{"event":"sms.received","from":"+15551234567","body":"Hello"}"#;
        let header = "sha256=b26c89d5db3df96f2dfe8504f5e5a7ce3807c5bd3eaf080d718c304eb9960ba8";

        assert!(verify_google_voice_signature(secret, payload, Some(header)).is_ok());
    }

    #[test]
    fn verify_google_voice_signature_rejects_mismatch_and_missing_header() {
        let secret = "test-secret";
        let payload = br#"{"event":"sms.received","from":"+15551234567","body":"Hello"}"#;

        assert!(verify_google_voice_signature(secret, payload, None).is_err());
        assert!(
            verify_google_voice_signature(secret, payload, Some("sha256=deadbeef")).is_err()
        );
        assert!(
            verify_google_voice_signature(secret, payload, Some("not-sha256=abc")).is_err()
        );
    }

    #[test]
    fn parse_bridge_json_sms_from_body_field() {
        let channel_id = uuid::Uuid::new_v4();
        let body = json!({
            "event": "sms.received",
            "from": "+15551234567",
            "body": "Hello there",
            "message_id": "msg-42"
        });

        let msgs = GoogleVoiceChannel::parse_bridge_json(channel_id, &body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].channel_id, channel_id);
        assert_eq!(msgs[0].sender_id, "+15551234567");
        assert_eq!(msgs[0].content, "Hello there");
        assert_eq!(
            msgs[0].metadata.get("google_voice_kind").and_then(|v| v.as_str()),
            Some("sms")
        );
        assert_eq!(
            msgs[0]
                .metadata
                .get("google_voice_message_id")
                .and_then(|v| v.as_str()),
            Some("msg-42")
        );
    }

    #[test]
    fn parse_bridge_json_call_event_and_empty_sms() {
        let channel_id = uuid::Uuid::new_v4();

        let call = json!({
            "event": "call.ringing",
            "from": "+15559876543",
            "status": "ringing"
        });
        let call_msgs = GoogleVoiceChannel::parse_bridge_json(channel_id, &call);
        assert_eq!(call_msgs.len(), 1);
        assert_eq!(call_msgs[0].content, "Google Voice call from +15559876543 (ringing)");
        assert_eq!(
            call_msgs[0]
                .metadata
                .get("google_voice_kind")
                .and_then(|v| v.as_str()),
            Some("call")
        );

        let empty = json!({ "event": "sms.received", "from": "+15550001111", "body": "" });
        assert!(GoogleVoiceChannel::parse_bridge_json(channel_id, &empty).is_empty());

        let alt_text = json!({ "from": "+15550002222", "text": "Via text field" });
        let alt_msgs = GoogleVoiceChannel::parse_bridge_json(channel_id, &alt_text);
        assert_eq!(alt_msgs.len(), 1);
        assert_eq!(alt_msgs[0].content, "Via text field");
    }
}
