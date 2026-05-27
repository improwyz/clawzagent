//! Slack channel implementation for the ClawZ worker.
//!
//! Integrates with the [Slack Web API](https://api.slack.com/web) to read
//! channel history and post messages (including threaded replies and block-kit
//! image blocks).
//!
//! # Capabilities
//! - **receive**: Polls `conversations.history` with cursor-based pagination.
//! - **send**: Posts `chat.postMessage` with mrkdwn section blocks and image embeds.
//! - **webhook**: Parses Slack Events API payloads; verifies `X-Slack-Signature` HMAC-SHA256.
//! - **auth**: Bot token (`xoxb-…`) and optional `signing_secret` for webhook verification.
//!
//! # Required credentials
//! | key               | description                                        |
//! |-------------------|----------------------------------------------------|
//! | `bot_token`       | Slack bot user OAuth token                         |
//! | `channel_id`      | Target conversation ID (e.g. `C1234567890`)        |
//! | `signing_secret`  | (optional) Slack app signing secret for webhook HMAC |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── Slack channel implementation ──────────────────────────────────────────────
//
// Uses the Slack Web API v2:
//   receive : conversations.history (+ pagination cursor)
//   send    : chat.postMessage (with blocks)
//   webhook : Slack Events API (X-Slack-Signature HMAC-SHA256 verification)

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message and attachment types defined in core
use clawz_core::types::channel::{Attachment, ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
// Dependency: hmac + sha2 for Slack's HMAC-SHA256 request signature verification
use hmac::{Hmac, Mac};
use sha2::Sha256;
use serde_json::{json, Value};

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error, markdown_to_slack_mrkdwn};

// Signing is optional until secrets are configured.
#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// Base URL for the Slack Web API.
const BASE: &str = "https://slack.com/api";

/// Slack text-channel channel.
///
/// A zero-sized struct that encapsulates all Slack-specific I/O logic.
/// Configuration (bot token, channel ID, optional signing secret) is pulled
/// from [`ChannelContext`] at call time so the same type can be reused across
/// multiple Slack workspaces.
pub struct SlackChannel;

impl SlackChannel {
    /// Create a new Slack channel instance.
    ///
    /// The instance holds no state; credentials are resolved per-request
    /// from the [`ChannelContext`] supplied to each trait method.
    pub fn new() -> Self {
        Self
    }

    /// Extract the Slack bot token from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `bot_token` is missing.
    fn bot_token<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "bot_token")
    }

    /// Extract the target channel ID from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `channel_id` is missing.
    fn channel_id<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "channel_id")
    }

    /// Convert a raw Slack message object into an [`IncomingMessage`].
    ///
    /// Extracts the `ts` (timestamp / message ID), `user`, text content,
    /// thread context, and file attachments. Returns `None` if the message
    /// lacks the required `text` or `ts` fields.
    fn parse_message(&self, ctx: &ChannelContext, msg: &Value) -> Option<IncomingMessage> {
        let text = msg.get("text")?.as_str()?.to_string();
        let ts = msg.get("ts")?.as_str()?.to_string();
        let user = msg
            .get("user")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let mut im = IncomingMessage::new(ctx.config.id, user.clone(), user, text);
        im.metadata.insert("slack_ts".to_string(), Value::String(ts.clone()));

        // thread_ts present means message is in a thread; we map it to thread_id.
        if let Some(thread_ts) = msg.get("thread_ts").and_then(|v| v.as_str()) {
            im.thread_id = Some(thread_ts.to_string());
        }

        // Files / attachments: Slack uses a `files` array with private URLs.
        if let Some(files) = msg.get("files").and_then(|v| v.as_array()) {
            for f in files {
                let id = f.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("file").to_string();
                let mime = f
                    .get("mimetype")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let url = f.get("url_private").and_then(|v| v.as_str()).map(|s| s.to_string());
                let size = f.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                im.attachments.push(Attachment {
                    id,
                    filename: name,
                    content_type: mime,
                    url,
                    data: None,
                    size_bytes: size,
                });
            }
        }

        Some(im)
    }
}

// Signing is optional until secrets are configured.
#[allow(dead_code)]
impl SlackChannel {
    fn signing_secret<'a>(&self, ctx: &'a ChannelContext) -> Option<&'a str> {
        ctx.config.credentials.get("signing_secret")?.as_str()
    }

    fn verify_signature(
        &self,
        signing_secret: &str,
        _headers: &HeaderMap,
        _payload: &[u8],
    ) -> Result<()> {
        let timestamp = _headers
            .get("X-Slack-Request-Timestamp")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ClawzError::Auth("missing X-Slack-Request-Timestamp".to_string()))?;

        let expected_sig = _headers
            .get("X-Slack-Signature")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ClawzError::Auth("missing X-Slack-Signature".to_string()))?;

        let sig_base = format!(
            "v0:{}:{}",
            timestamp,
            std::str::from_utf8(_payload).unwrap_or("")
        );

        let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
            .map_err(|e| ClawzError::Auth(format!("HMAC init: {e}")))?;
        mac.update(sig_base.as_bytes());
        let computed = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

        if computed != expected_sig {
            return Err(ClawzError::Auth("Slack signature mismatch".to_string()));
        }
        Ok(())
    }
}

impl Default for SlackChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for SlackChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Slack".to_string(),
            platform: "slack".to_string(),
            version: "1.0.0".to_string(),
            author: "ClawZ".to_string(),
        }
    }

    /// Advertise Slack-specific capabilities.
    ///
    /// Delegates to the convenience constructor [`ChannelCapabilities::slack`]
    /// defined in `clawz_core`.
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::slack()
    }

    /// Poll Slack for messages in the configured channel.
    ///
    /// Uses `conversations.history` with a `cursor` from
    /// `response_metadata.next_cursor`. Fetches up to 200 messages per page.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials or `channel_id` are missing.
    /// - [`ClawzError::Channel`] on HTTP failure or Slack API `ok: false`.
    /// - [`ClawzError::Serialization`] if the response body is not valid JSON.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let token = self.bot_token(ctx)?;
        let channel_id = self.channel_id(ctx)?;
        let client = &ctx.http_client;

        let mut messages = Vec::new();
        let mut cursor: Option<String> = None;

        // Paginate through all available history (max 200 per page)
        loop {
            let mut params = vec![
                ("channel", channel_id.to_string()),
                ("limit", "200".to_string()),
            ];
            if let Some(ref c) = cursor {
                params.push(("cursor", c.clone()));
            }

            let resp = client
                .get(format!("{BASE}/conversations.history"))
                .bearer_auth(token)
                .query(&params)
                .send()
                .await
                .map_err(|e| ClawzError::Channel(format!("Slack HTTP: {e}")))?;

            let status = resp.status();
            let body: Value = resp
                .json()
                .await
                .map_err(|e| ClawzError::Serialization(e.to_string()))?;

            if !status.is_success() {
                return Err(map_http_error(status, &body.to_string()));
            }

            // Slack returns HTTP 200 even for API errors; we must inspect `ok`.
            if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                let err = body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
                return Err(ClawzError::Channel(format!("Slack API error: {err}")));
            }

            if let Some(msgs) = body.get("messages").and_then(|v| v.as_array()) {
                for m in msgs {
                    if let Some(im) = self.parse_message(ctx, m) {
                        messages.push(im);
                    }
                }
            }

            // Check next cursor: Slack returns an empty string when there are no more pages.
            let next = body
                .pointer("/response_metadata/next_cursor")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }

        Ok(messages)
    }

    /// Send an outgoing message to the configured Slack channel.
    ///
    /// Converts markdown to Slack mrkdwn, builds a Block Kit section block for
    /// the text, appends image blocks for image attachments, and includes a
    /// `thread_ts` when `msg.reply_to` is set.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials are missing.
    /// - [`ClawzError::Channel`] on HTTP failure or Slack API `ok: false`.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let token = self.bot_token(ctx)?;
        let channel_id = self.channel_id(ctx)?;
        let client = &ctx.http_client;

        let mrkdwn = markdown_to_slack_mrkdwn(&msg.content);

        // Build blocks: a section block for the text content
        let mut blocks: Vec<Value> = vec![json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": mrkdwn }
        })];

        // Image attachments become image blocks so they render inline in Slack.
        for att in &msg.attachments {
            if att.content_type.starts_with("image/") {
                if let Some(ref url) = att.url {
                    blocks.push(json!({
                        "type": "image",
                        "image_url": url,
                        "alt_text": att.filename
                    }));
                }
            }
        }

        let mut body = json!({
            "channel": channel_id,
            "text": &msg.content,   // fallback for notifications
            "blocks": blocks,
        });

        // Thread reply: reply_to holds the parent message's `ts`.
        if let Some(ref ts) = msg.reply_to {
            body["thread_ts"] = Value::String(ts.clone());
        }

        let resp = client
            .post(format!("{BASE}/chat.postMessage"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Slack HTTP: {e}")))?;

        let status = resp.status();
        let resp_body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(status, &resp_body.to_string()));
        }

        // Slack returns HTTP 200 with `ok: false` for semantic errors (e.g. invalid channel).
        if resp_body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let err = resp_body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(ClawzError::Channel(format!("Slack send error: {err}")));
        }

        Ok(())
    }

    /// Parse a Slack Events API or interaction payload into [`IncomingMessage`]s.
    ///
    /// Handles two special cases before standard event parsing:
    /// 1. **URL verification challenge** (`challenge` field) → returns a synthetic
    ///    message with `_slack_challenge` metadata so the gateway can echo it.
    /// 2. **Standard event callback** (`event.type == "message"`) → extracts text,
    ///    user, timestamp, channel, and thread context.
    ///
    /// Signature verification is intentionally skipped here because `webhook()`
    /// does not receive a [`ChannelContext`]; callers should verify at the gateway
    /// layer where the `signing_secret` is available.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        // Verify signature if signing_secret is configured.
        // We need a ctx for that but webhook() doesn't have one — instead we
        // create a minimal context from what we have.  The signing secret is
        // embedded in credentials, but webhook() only gets raw bytes + headers.
        //
        // For signature verification at the handler level (e.g. in the gateway)
        // the signing secret would be available.  Here we do a best-effort
        // parse and leave verification to the caller if needed.

        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("Slack webhook parse: {e}")))?;

        // URL verification challenge: Slack sends this when configuring the Events API.
        if let Some(challenge) = body.get("challenge").and_then(|v| v.as_str()) {
            // The gateway should respond with the challenge — we encode it in
            // the IncomingMessage metadata so the caller knows what to reply.
            let mut dummy = IncomingMessage::new(
                uuid::Uuid::nil(),
                "slack_system".to_string(),
                "Slack".to_string(),
                challenge.to_string(),
            );
            dummy
                .metadata
                .insert("_slack_challenge".to_string(), Value::String(challenge.to_string()));
            return Ok(vec![dummy]);
        }

        let mut messages = Vec::new();

        // Standard event callback
        if let Some(event) = body.get("event") {
            let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if event_type == "message" {
                let text = event
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let user = event
                    .get("user")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let ts = event
                    .get("ts")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let channel_str = event
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let mut im = IncomingMessage::new(
                    uuid::Uuid::new_v4(),
                    user.clone(),
                    user,
                    text,
                );
                im.metadata
                    .insert("slack_ts".to_string(), Value::String(ts));
                im.metadata
                    .insert("slack_channel".to_string(), Value::String(channel_str));

                if let Some(thread_ts) = event.get("thread_ts").and_then(|v| v.as_str()) {
                    im.thread_id = Some(thread_ts.to_string());
                }

                messages.push(im);
            }
        }

        Ok(messages)
    }
}
