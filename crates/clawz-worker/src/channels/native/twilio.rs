//! Twilio SMS and voice channel for the ClawZ worker.
//!
//! Uses the [Twilio REST API](https://www.twilio.com/docs/usage/api) (2010-04-01)
//! for outbound SMS/calls and parses inbound webhooks (form-urlencoded).
//!
//! # Required credentials
//! | key | description |
//! |-----|-------------|
//! | `account_sid` | Twilio Account SID |
//! | `auth_token` | Twilio Auth Token |
//! | `phone_number` | E.164 number on the account (From) |
//!
//! # Optional
//! | key | description |
//! |-----|-------------|
//! | `agent_id` | Bound agent (used by gateway webhooks) |
//! | `voice_twiml_url` | TwiML URL for outbound calls (`Url` param) |
//! | `default_to` | Default SMS/call recipient |

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
use clawz_core::types::channel::{ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::channels::plugin::{cred_str, map_http_error};

/// Twilio REST API base (per account).
fn api_base(account_sid: &str) -> String {
    format!("https://api.twilio.com/2010-04-01/Accounts/{account_sid}")
}

/// Twilio phone channel (SMS + voice webhooks).
pub struct TwilioChannel;

impl TwilioChannel {
    pub fn new() -> Self {
        Self
    }

    fn phone_number(ctx: &ChannelContext) -> Result<&str> {
        cred_str(&ctx.config.credentials, "phone_number")
    }

    fn authed_get(
        ctx: &ChannelContext,
        path: &str,
        query: &[(&str, String)],
    ) -> reqwest::RequestBuilder {
        let sid = ctx
            .config
            .credentials
            .get("account_sid")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let token = ctx
            .config
            .credentials
            .get("auth_token")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut req = ctx
            .http_client
            .get(format!("{}{}", api_base(sid), path))
            .basic_auth(sid, Some(token));
        if !query.is_empty() {
            req = req.query(query);
        }
        req
    }

    fn authed_post_form(
        ctx: &ChannelContext,
        path: &str,
        params: &[(&str, &str)],
    ) -> reqwest::RequestBuilder {
        let sid = ctx
            .config
            .credentials
            .get("account_sid")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let token = ctx
            .config
            .credentials
            .get("auth_token")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        ctx.http_client
            .post(format!("{}{}", api_base(sid), path))
            .basic_auth(sid, Some(token))
            .form(params)
    }

    /// Parse `application/x-www-form-urlencoded` Twilio webhook bodies.
    pub fn parse_form_webhook(payload: &[u8]) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for pair in String::from_utf8_lossy(payload).split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(decode_form_component(k), decode_form_component(v));
            }
        }
        map
    }

    fn form_to_messages(
        channel_id: uuid::Uuid,
        form: &HashMap<String, String>,
    ) -> Vec<IncomingMessage> {
        let body = form.get("Body").cloned().unwrap_or_default();
        let speech = form.get("SpeechResult").cloned().unwrap_or_default();
        let text = if !body.is_empty() {
            body.clone()
        } else {
            speech.clone()
        };

        if text.is_empty() && form.get("CallSid").is_none() {
            return Vec::new();
        }

        let from = form
            .get("From")
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let to = form.get("To").cloned().unwrap_or_default();
        let msg_sid = form
            .get("MessageSid")
            .or_else(|| form.get("CallSid"))
            .cloned()
            .unwrap_or_default();

        let display = if text.is_empty() {
            format!(
                "Call {} ({})",
                form.get("CallStatus").cloned().unwrap_or_default(),
                msg_sid
            )
        } else {
            text.clone()
        };

        let mut im = IncomingMessage::new(channel_id, from.clone(), from, display);
        im.metadata.insert("twilio_sid".into(), json!(msg_sid));
        im.metadata.insert("twilio_to".into(), json!(to));
        if !body.is_empty() {
            im.metadata
                .insert("twilio_kind".into(), Value::String("sms".into()));
        } else if form.contains_key("CallSid") {
            im.metadata
                .insert("twilio_kind".into(), Value::String("voice".into()));
            if let Some(status) = form.get("CallStatus") {
                im.metadata
                    .insert("twilio_call_status".into(), json!(status));
            }
        }
        if !speech.is_empty() {
            im.metadata
                .insert("twilio_speech_result".into(), json!(speech));
        }

        vec![im]
    }
}

impl Default for TwilioChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for TwilioChannel {
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Twilio".into(),
            platform: "twilio".into(),
            version: "1.0.0".into(),
            author: "ClawZ".into(),
        }
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            media: true,
            typing: false,
            reactions: false,
            threads: false,
            voice: true,
            video: false,
            file_upload: false,
        }
    }

    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let phone = Self::phone_number(ctx)?;
        let resp = Self::authed_get(ctx, "/Messages.json", &[("PageSize", "50".into())])
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Twilio list messages: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            let fallback = body.to_string();
            return Err(map_http_error(status, fallback.as_str()));
        }

        let items = body
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut out = Vec::new();
        for item in items {
            let direction = item
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if direction != "inbound" {
                continue;
            }
            let to = item.get("to").and_then(|v| v.as_str()).unwrap_or("");
            if to != phone {
                continue;
            }
            let from = item
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let text = item
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sid = item
                .get("sid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut im = IncomingMessage::new(ctx.config.id, from.clone(), from, text);
            im.metadata
                .insert("twilio_sid".into(), Value::String(sid));
            im.metadata
                .insert("twilio_kind".into(), Value::String("sms".into()));
            out.push(im);
        }
        Ok(out)
    }

    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let from = Self::phone_number(ctx)?;
        let action = msg
            .metadata
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("sms");

        match action {
            "call" | "voice" => {
                let to = msg
                    .metadata
                    .get("to")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        ctx.config
                            .credentials
                            .get("default_to")
                            .and_then(|v| v.as_str())
                    })
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ClawzError::Config("Twilio call: missing metadata.to or default_to".into())
                    })?;

                let url = msg
                    .metadata
                    .get("twiml_url")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        ctx.config
                            .credentials
                            .get("voice_twiml_url")
                            .and_then(|v| v.as_str())
                    })
                    .ok_or_else(|| {
                        ClawzError::Config(
                            "Twilio call requires voice_twiml_url in credentials or metadata"
                                .into(),
                        )
                    })?;

                let params = [("To", to), ("From", from), ("Url", url)];
                let resp = Self::authed_post_form(ctx, "/Calls.json", &params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Channel(format!("Twilio call: {e}")))?;
                let status = resp.status();
                if !status.is_success() {
                    let text = resp.text().await.unwrap_or_default();
                    return Err(map_http_error(status, &text));
                }
                Ok(())
            }
            _ => {
                let to = msg
                    .metadata
                    .get("to")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        ctx.config
                            .credentials
                            .get("default_to")
                            .and_then(|v| v.as_str())
                    })
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ClawzError::Config("Twilio SMS: missing metadata.to or default_to".into())
                    })?;

                let params = [
                    ("To", to),
                    ("From", from),
                    ("Body", msg.content.as_str()),
                ];
                let resp = Self::authed_post_form(ctx, "/Messages.json", &params)
                    .send()
                    .await
                    .map_err(|e| ClawzError::Channel(format!("Twilio SMS: {e}")))?;
                let status = resp.status();
                if !status.is_success() {
                    let text = resp.text().await.unwrap_or_default();
                    return Err(map_http_error(status, &text));
                }
                Ok(())
            }
        }
    }

    async fn webhook(&self, payload: &[u8], headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let _ = headers;
        if payload.first() == Some(&b'{') {
            let body: Value = serde_json::from_slice(payload)
                .map_err(|e| ClawzError::Serialization(format!("Twilio JSON webhook: {e}")))?;
            let from = body
                .get("From")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let text = body
                .get("Body")
                .or_else(|| body.get("SpeechResult"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return Ok(Vec::new());
            }
            let mut im = IncomingMessage::new(uuid::Uuid::new_v4(), from.clone(), from, text);
            im.metadata
                .insert("twilio_kind".into(), Value::String("sms".into()));
            return Ok(vec![im]);
        }

        let form = Self::parse_form_webhook(payload);
        Ok(Self::form_to_messages(uuid::Uuid::new_v4(), &form))
    }
}

fn decode_form_component(s: &str) -> String {
    let replaced = s.replace('+', " ");
    let mut out = String::new();
    let bytes = replaced.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sms_webhook_form() {
        let raw = b"MessageSid=SM1&From=%2B15551111111&To=%2B15552222222&Body=Hello";
        let form = TwilioChannel::parse_form_webhook(raw);
        let msgs = TwilioChannel::form_to_messages(uuid::Uuid::new_v4(), &form);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Hello");
    }
}
