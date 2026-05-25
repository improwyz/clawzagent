//! Dialpad channel implementation for the ClawZ worker.
//!
//! Integrates with the [Dialpad REST API v2](https://developers.dialpad.com/)
//! to send and receive SMS messages on a configured phone number.
//!
//! # Capabilities
//! - **receive**: Polls `GET /api/v2/sms` for inbound SMS (optionally filtered by direction).
//! - **send**: Posts `POST /api/v2/sms` to dispatch text messages.
//! - **webhook**: Parses Dialpad webhook events (`sms`, `message`, etc.) into canonical messages.
//! - **auth**: API key passed as a Bearer token.
//!
//! # Required credentials
//! | key            | description                              |
//! |----------------|------------------------------------------|
//! | `api_key`      | Dialpad API key (Bearer token)             |
//! | `phone_number` | Registered Dialpad number (E.164 format) |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── Dialpad channel implementation ────────────────────────────────────────────
//
// Uses the Dialpad API v2:
//   receive : GET /api/v2/sms (list received SMS messages)
//   send    : POST /api/v2/sms
//   webhook : Parse Dialpad webhook event (call, sms, voicemail, etc.)
//   auth    : API key as Bearer token

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message types defined in core
use clawz_core::types::channel::{ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use serde_json::{json, Value};

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error};

/// Base URL for the Dialpad REST API v2.
const BASE: &str = "https://dialpad.com/api/v2";

/// Dialpad SMS channel.
///
/// A zero-sized struct that encapsulates all Dialpad-specific I/O logic.
/// Configuration (API key, phone number) is pulled from [`ChannelContext`]
/// at call time so the same type can be reused across multiple Dialpad accounts.
pub struct DialpadChannel;

impl DialpadChannel {
    /// Create a new Dialpad channel instance.
    ///
    /// The instance holds no state; credentials are resolved per-request
    /// from the [`ChannelContext`] supplied to each trait method.
    pub fn new() -> Self {
        Self
    }

    /// Extract the Dialpad API key from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `api_key` is missing.
    fn api_key<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "api_key")
    }

    /// Extract the registered phone number from channel credentials.
    ///
    /// Used as the `from_number` when sending and as a filter when polling.
    /// # Errors
    /// Returns [`ClawzError::Config`] if `phone_number` is missing.
    fn phone_number<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "phone_number")
    }
}

impl Default for DialpadChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for DialpadChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Dialpad".into(),
            platform: "dialpad".into(),
            version: "1.0.0".into(),
            author: "ClawZ".into(),
        }
    }

    /// Advertise Dialpad-specific capabilities.
    ///
    /// Dialpad supports voice calls natively but this channel implementation
    /// currently focuses on SMS; voice is marked `true` for future expansion.
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

    /// Poll Dialpad for inbound SMS messages.
    ///
    /// Sends `GET /api/v2/sms` filtered to `direction=inbound` and limited to
    /// 50 results. Each returned item is mapped to an [`IncomingMessage`].
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    /// - [`ClawzError::Serialization`] if the response body is not valid JSON.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let api_key = self.api_key(ctx)?;
        let phone_number = self.phone_number(ctx)?;
        let client = &ctx.http_client;

        // Dialpad SMS list endpoint
        let mut params = vec![
            ("phone_number", phone_number.to_string()),
            ("limit", "50".into()),
        ];

        // Restrict to inbound-only so we don't echo messages we already sent.
        params.push(("direction", "inbound".into()));

        let resp = client
            .get(format!("{BASE}/sms"))
            .bearer_auth(api_key)
            .query(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Dialpad HTTP: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(status, &body.to_string()));
        }

        let mut messages = Vec::new();

        // Dialpad returns { "items": [...], "cursor": ... }
        let items = body
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for item in &items {
            let sms_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // `from_number` is the caller's number; we use it as both ID and display name.
            let from = item
                .get("from_number")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let mut im = IncomingMessage::new(
                ctx.config.id,
                from.clone(),
                from,
                text,
            );
            // Preserve the upstream Dialpad message ID for idempotency / threading later.
            im.metadata
                .insert("dialpad_sms_id".into(), Value::String(sms_id));
            messages.push(im);
        }

        Ok(messages)
    }

    /// Send an outgoing SMS via Dialpad.
    ///
    /// The recipient phone number is resolved in this priority:
    /// 1. `msg.metadata["to"]`
    /// 2. `ctx.config.credentials["default_recipient"]`
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if no recipient can be determined.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let api_key = self.api_key(ctx)?;
        let phone_number = self.phone_number(ctx)?;
        let client = &ctx.http_client;

        // Resolve destination with fallback to default_recipient for convenience.
        let to = msg
            .metadata
            .get("to")
            .and_then(|v| v.as_str())
            .or_else(|| {
                ctx.config
                    .credentials
                    .get("default_recipient")
                    .and_then(|v| v.as_str())
            })
            .ok_or_else(|| ClawzError::Config("Dialpad: missing 'to' phone number".into()))?
            .to_string();

        let body = json!({
            "to_numbers": [to],
            "from_number": phone_number,
            "text": &msg.content,
            "infer_country_code": true,
        });

        let resp = client
            .post(format!("{BASE}/sms"))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Dialpad send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a Dialpad webhook payload into one or more [`IncomingMessage`]s.
    ///
    /// Only `event == "sms" | "message"` is processed; other events (calls,
    /// voicemails) are silently ignored and return an empty vector.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("Dialpad webhook: {e}")))?;

        let event_type = body
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Guard: ignore non-SMS webhook events (calls, voicemails, etc.)
        if event_type != "sms" && event_type != "message" {
            return Ok(Vec::new());
        }

        let sms_id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let text = body
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let from = body
            .get("from_number")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let mut im = IncomingMessage::new(
            uuid::Uuid::new_v4(),
            from.clone(),
            from,
            text,
        );
        im.metadata
            .insert("dialpad_sms_id".into(), Value::String(sms_id));
        // Tag the original event type so downstream logic can distinguish sms vs message.
        im.metadata
            .insert("dialpad_event".into(), Value::String(event_type.to_string()));

        Ok(vec![im])
    }
}
