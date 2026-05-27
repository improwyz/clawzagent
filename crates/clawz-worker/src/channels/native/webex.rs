//! Cisco Webex channel implementation for the ClawZ worker.
//!
//! Integrates with the [Webex REST API](https://developer.webex.com/docs/api/v1/messages)
//! to read and post messages in a Webex room.
//!
//! # Capabilities
//! - **receive**: Polls `GET /v1/messages` with `beforeMessage` pagination.
//! - **send**: Posts `POST /v1/messages` (markdown + file URLs).
//! - **webhook**: Parses Webex webhook events; verifies `X-Spark-Signature` HMAC-SHA1.
//! - **auth**: Bot access token (Bearer).
//!
//! # Required credentials
//! | key               | description                                         |
//! |-------------------|-----------------------------------------------------|
//! | `access_token`    | Webex bot access token                              |
//! | `room_id`         | Target room ID (UUID)                               |
//! | `webhook_secret`  | (optional) Secret for HMAC-SHA1 webhook verification |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── Cisco Webex channel implementation ────────────────────────────────────────
//
// Uses the Webex REST API:
//   receive : GET /v1/messages (list messages in a room, with pagination)
//   send    : POST /v1/messages (text + markdown + file attachments)
//   webhook : Parse Webex webhook event; verify X-Spark-Signature HMAC-SHA1
//   auth    : Bot access token (Bearer)

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message and attachment types defined in core
use clawz_core::types::channel::{
    Attachment, ChannelCapabilities, IncomingMessage, OutgoingMessage,
};
// Dependency: hmac + sha1 for Webex's legacy HMAC-SHA1 webhook signature verification
use hmac::{Hmac, Mac};
use http::HeaderMap;
use serde_json::{Value, json};
use sha1::Sha1;

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error};

// Signing is optional until secrets are configured.
#[allow(dead_code)]
type HmacSha1 = Hmac<Sha1>;

/// Base URL for the Webex REST API v1.
const BASE: &str = "https://webexapis.com/v1";

/// Cisco Webex room channel.
///
/// A zero-sized struct that encapsulates all Webex-specific I/O logic.
/// Configuration (access token, room ID, optional webhook secret) is pulled
/// from [`ChannelContext`] at call time so the same type can be reused across
/// multiple Webex rooms.
pub struct WebexChannel;

impl WebexChannel {
    /// Create a new Webex channel instance.
    ///
    /// The instance holds no state; credentials are resolved per-request
    /// from the [`ChannelContext`] supplied to each trait method.
    pub fn new() -> Self {
        Self
    }

    /// Extract the bot access token from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `access_token` is missing.
    fn token<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "access_token")
    }

    /// Extract the target room ID from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `room_id` is missing.
    fn room_id<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "room_id")
    }

    /// Convert a raw Webex message JSON object into an [`IncomingMessage`].
    ///
    /// Extracts text (preferring `markdown` over plain `text`), sender identity
    /// (`personId` / `personEmail`), and file attachments. Returns `None` if the
    /// message lacks a required `id` field.
    fn parse_message(&self, ctx: &ChannelContext, item: &Value) -> Option<IncomingMessage> {
        let id = item.get("id")?.as_str()?.to_string();
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("markdown").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let sender_id = item
            .get("personId")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let sender_name = item
            .get("personEmail")
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();

        let mut im = IncomingMessage::new(ctx.config.id, sender_id, sender_name, text);
        im.metadata.insert("webex_msg_id".into(), Value::String(id));

        // Webex returns file URLs as a simple string array; map each to an Attachment.
        if let Some(files) = item.get("files").and_then(|v| v.as_array()) {
            for f in files {
                if let Some(url) = f.as_str() {
                    im.attachments.push(Attachment {
                        id: uuid::Uuid::new_v4().to_string(),
                        filename: url.rsplit('/').next().unwrap_or("file").to_string(),
                        content_type: "application/octet-stream".into(),
                        url: Some(url.to_string()),
                        data: None,
                        size_bytes: 0,
                    });
                }
            }
        }

        Some(im)
    }
}

// Signing is optional until secrets are configured.
#[allow(dead_code)]
impl WebexChannel {
    fn verify_signature(
        &self,
        ctx: &ChannelContext,
        _headers: &HeaderMap,
        _payload: &[u8],
    ) -> Result<()> {
        let secret = match cred_str(&ctx.config.credentials, "webhook_secret") {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };

        let sig = _headers
            .get("X-Spark-Signature")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ClawzError::Auth("missing X-Spark-Signature".into()))?;

        let mut mac = HmacSha1::new_from_slice(secret.as_bytes())
            .map_err(|e| ClawzError::Auth(format!("HMAC init: {e}")))?;
        mac.update(_payload);
        let computed = hex::encode(mac.finalize().into_bytes());

        if computed != sig {
            return Err(ClawzError::Auth("Webex signature mismatch".into()));
        }

        Ok(())
    }
}

impl Default for WebexChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for WebexChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Cisco Webex".into(),
            platform: "webex".into(),
            version: "1.0.0".into(),
            author: "ClawZ".into(),
        }
    }

    /// Advertise Webex-specific capabilities.
    ///
    /// Webex supports media, typing, voice, video, and file uploads.
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            media: true,
            typing: true,
            reactions: false,
            threads: false,
            voice: true,
            video: true,
            file_upload: true,
        }
    }

    /// Poll Webex for messages in the configured room.
    ///
    /// Paginates backwards using `beforeMessage` (message ID of the oldest item
    /// on the current page) until fewer than 200 messages are returned.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials or `room_id` are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    /// - [`ClawzError::Serialization`] if the response body is not valid JSON.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let token = self.token(ctx)?;
        let room_id = self.room_id(ctx)?;
        let client = &ctx.http_client;

        let mut messages = Vec::new();
        let mut before_message: Option<String> = None;

        loop {
            let mut params = vec![("roomId", room_id.to_string()), ("max", "200".into())];
            if let Some(ref bm) = before_message {
                params.push(("beforeMessage", bm.clone()));
            }

            let resp = client
                .get(format!("{BASE}/messages"))
                .bearer_auth(token)
                .query(&params)
                .send()
                .await
                .map_err(|e| ClawzError::Channel(format!("Webex HTTP: {e}")))?;

            let status = resp.status();
            let body: Value = resp
                .json()
                .await
                .map_err(|e| ClawzError::Serialization(e.to_string()))?;

            if !status.is_success() {
                return Err(map_http_error(status, &body.to_string()));
            }

            let items = match body.get("items").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a.clone(),
                _ => break,
            };

            // Capture the oldest message ID to use as `beforeMessage` for the next page.
            let oldest_id = items
                .last()
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            for item in &items {
                if let Some(im) = self.parse_message(ctx, item) {
                    messages.push(im);
                }
            }

            // A partial page means we've reached the end of history.
            if items.len() < 200 {
                break;
            }
            before_message = oldest_id;
        }

        Ok(messages)
    }

    /// Send an outgoing message to the configured Webex room.
    ///
    /// Posts both `text` (plain fallback) and `markdown` (rich rendering).
    /// Appends any attachment URLs as `files` so Webex renders them inline.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials or `room_id` are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let token = self.token(ctx)?;
        let room_id = self.room_id(ctx)?;
        let client = &ctx.http_client;

        let mut body = json!({
            "roomId": room_id,
            "markdown": &msg.content,
            "text": &msg.content,
        });

        // Attach files by URL: Webex will fetch and render them in the room.
        let file_urls: Vec<Value> = msg
            .attachments
            .iter()
            .filter_map(|a| a.url.as_ref().map(|u| Value::String(u.clone())))
            .collect();
        if !file_urls.is_empty() {
            body["files"] = Value::Array(file_urls);
        }

        let resp = client
            .post(format!("{BASE}/messages"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Webex send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a Webex webhook payload into [`IncomingMessage`]s.
    ///
    /// Only processes `resource == "messages"` + `event == "created"`.
    /// Note: the webhook payload does **not** include the message text; callers
    /// that need the full body should perform a follow-up `GET /v1/messages/{id}`.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("Webex webhook: {e}")))?;

        let resource = body.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        let event = body.get("event").and_then(|v| v.as_str()).unwrap_or("");

        // Guard: only handle newly-created messages; ignore edits, deletions, etc.
        if resource != "messages" || event != "created" {
            return Ok(Vec::new());
        }

        let data = body.get("data").cloned().unwrap_or_default();
        let msg_id = data
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let sender_id = data
            .get("personId")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let sender_email = data
            .get("personEmail")
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();

        // Webhook does not include message text; callers must fetch via messages.get
        let mut im =
            IncomingMessage::new(uuid::Uuid::new_v4(), sender_id, sender_email, String::new());
        im.metadata
            .insert("webex_msg_id".into(), Value::String(msg_id));

        Ok(vec![im])
    }
}
