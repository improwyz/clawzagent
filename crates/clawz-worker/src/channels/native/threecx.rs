//! 3CX channel implementation for the ClawZ worker.
//!
//! Integrates with the [3CX REST API](https://www.3cx.com/docs/manual/) to
//! send and receive chat messages on a 3CX PBX instance.
//!
//! # Capabilities
//! - **receive**: Polls `GET /api/chat/messages` (optionally filtered by queue ID).
//! - **send**: Posts `POST /api/chat/messages` to a party or queue extension.
//! - **webhook**: Parses 3CX webhook events (`chat.message`, etc.).
//! - **auth**: Either `X-Api-Key` header or Basic auth (`username` + `password`).
//!
//! # Required credentials
//! | key         | description                                         |
//! |-------------|-----------------------------------------------------|
//! | `base_url`  | Root URL of the 3CX instance (e.g. `https://pbx.example.com`) |
//! | `api_key`   | API key for `X-Api-Key` header                      |
//! | `username`  | (optional) Basic auth username                      |
//! | `password`  | (optional) Basic auth password                      |
//! | `queue_id`  | (optional) Filter inbound messages to this queue    |
//! | `default_to`| (optional) Fallback recipient for outgoing messages |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── 3CX channel implementation ────────────────────────────────────────────────
//
// Uses the 3CX REST API:
//   receive : Poll GET /api/chat/messages (API key auth)
//   send    : POST /api/chat/messages
//   webhook : Parse 3CX webhook event (chat.message, call.answered, etc.)
//   auth    : API key header (X-API-Key or bearer depending on version)

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

/// 3CX chat channel.
///
/// A zero-sized struct that encapsulates all 3CX-specific I/O logic.
/// Configuration (base URL, API key, optional Basic auth) is pulled from
/// [`ChannelContext`] at call time so the same type can be reused across
/// multiple 3CX instances.
pub struct ThreeCXChannel;

impl ThreeCXChannel {
    /// Create a new 3CX channel instance.
    ///
    /// The instance holds no state; credentials are resolved per-request
    /// from the [`ChannelContext`] supplied to each trait method.
    pub fn new() -> Self {
        Self
    }

    /// Extract the 3CX instance base URL from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `base_url` is missing.
    fn base_url<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "base_url")
    }

    /// Extract the API key from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `api_key` is missing and no Basic auth is configured.
    fn api_key<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "api_key")
    }

    /// Apply the appropriate authentication scheme to an outgoing request.
    ///
    /// 3CX deployments vary: some use `X-Api-Key`, others use Basic auth.
    /// If both `username` and `password` are present in credentials, Basic
    /// auth takes precedence; otherwise the `X-Api-Key` header is used.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if neither Basic auth nor `api_key` is available.
    fn auth_request(
        &self,
        ctx: &ChannelContext,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder> {
        let api_key = self.api_key(ctx)?;
        // Check if credentials contain username (Basic auth)
        if let (Some(user), Some(pass)) = (
            ctx.config.credentials.get("username").and_then(|v| v.as_str()),
            ctx.config.credentials.get("password").and_then(|v| v.as_str()),
        ) {
            return Ok(builder.basic_auth(user, Some(pass)));
        }
        Ok(builder.header("X-Api-Key", api_key))
    }
}

impl Default for ThreeCXChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for ThreeCXChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "3CX".into(),
            platform: "3cx".into(),
            version: "1.0.0".into(),
            author: "ClawZ".into(),
        }
    }

    /// Advertise 3CX-specific capabilities.
    ///
    /// 3CX supports voice calls natively; this channel focuses on chat.
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

    /// Poll 3CX for chat messages.
    ///
    /// Sends `GET /api/chat/messages` with optional `queueId` filter and
    /// page size of 100. The response shape varies by 3CX version; we accept
    /// either `{ "data": [...] }` or a top-level array.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    /// - [`ClawzError::Serialization`] if the response body is not valid JSON.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let base_url = self.base_url(ctx)?;
        let client = &ctx.http_client;

        // Optional channel/queue ID filter
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(queue_id) = ctx.config.credentials.get("queue_id").and_then(|v| v.as_str()) {
            params.push(("queueId", queue_id.to_string()));
        }
        params.push(("pageSize", "100".into()));

        let req = client.get(format!("{base_url}/api/chat/messages")).query(&params);
        let req = self.auth_request(ctx, req)?;

        let resp = req
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("3CX HTTP: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(status, &body.to_string()));
        }

        let mut messages = Vec::new();

        // 3CX returns { "data": [...] } or just an array depending on version
        let items = body
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .or_else(|| body.as_array().cloned())
            .unwrap_or_default();

        for item in &items {
            let msg_id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Different 3CX versions use `body` or `message` for the text field.
            let text = item
                .get("body")
                .or_else(|| item.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Different 3CX versions use `from` or `sender` for the sender field.
            let sender_id = item
                .get("from")
                .or_else(|| item.get("sender"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let mut im = IncomingMessage::new(
                ctx.config.id,
                sender_id.clone(),
                sender_id,
                text,
            );
            im.metadata
                .insert("threecx_msg_id".into(), Value::String(msg_id));
            messages.push(im);
        }

        Ok(messages)
    }

    /// Send a chat message via 3CX.
    ///
    /// The recipient (`to`) is resolved in this priority:
    /// 1. `msg.metadata["to"]`
    /// 2. `ctx.config.credentials["default_to"]`
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let base_url = self.base_url(ctx)?;
        let client = &ctx.http_client;

        // Target: party or queue extension
        let to = msg
            .metadata
            .get("to")
            .and_then(|v| v.as_str())
            .or_else(|| ctx.config.credentials.get("default_to").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        let body = json!({
            "to": to,
            "body": &msg.content,
        });

        let req = client.post(format!("{base_url}/api/chat/messages")).json(&body);
        let req = self.auth_request(ctx, req)?;

        let resp = req
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("3CX send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a 3CX webhook payload into [`IncomingMessage`]s.
    ///
    /// Only processes events whose name contains `"chat"` or `"message"`;
    /// other events (calls, system) are ignored.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("3CX webhook: {e}")))?;

        let event = body
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Only handle chat message events; ignore call events, etc.
        if !event.contains("chat") && !event.contains("message") {
            return Ok(Vec::new());
        }

        let data = body.get("data").cloned().unwrap_or_default();

        let msg_id = data
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let text = data
            .get("body")
            .or_else(|| data.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let sender_id = data
            .get("from")
            .or_else(|| data.get("sender"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let mut im = IncomingMessage::new(
            uuid::Uuid::new_v4(),
            sender_id.clone(),
            sender_id,
            text,
        );
        im.metadata
            .insert("threecx_msg_id".into(), Value::String(msg_id));
        // Preserve the original event name for downstream filtering / logging.
        im.metadata
            .insert("threecx_event".into(), Value::String(event.to_string()));

        Ok(vec![im])
    }
}
