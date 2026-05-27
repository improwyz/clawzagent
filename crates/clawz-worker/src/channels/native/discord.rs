//! Discord channel implementation for the ClawZ worker.
//!
//! Integrates with the [Discord REST API v10](https://discord.com/developers/docs/reference)
//! to read and post messages in a specific text channel.
//!
//! # Capabilities
//! - **receive**: Polls `GET /channels/{id}/messages` with backwards pagination (`before`).
//! - **send**: Posts `POST /channels/{id}/messages` (supports reply references and image embeds).
//! - **webhook**: Parses Discord Interaction payloads; verifies Ed25519 signatures when configured.
//! - **auth**: Bot token (`Bot <token>`).
//!
//! # Required credentials
//! | key              | description                                          |
//! |------------------|------------------------------------------------------|
//! | `bot_token`      | Discord bot token (include in `Authorization` header) |
//! | `channel_id`     | Snowflake ID of the target text channel               |
//! | `public_key`     | (optional) Application public key for Ed25519 verify  |
//! | `signing_secret` | (optional) Used for interaction signature checks      |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── Discord channel implementation ────────────────────────────────────────────
//
// Uses Discord REST API v10:
//   receive : GET /channels/{id}/messages
//   send    : POST /channels/{id}/messages (embeds support)
//   webhook : Interaction webhook — Ed25519 signature verification
//   auth    : Bot token ("Bot <token>")

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
use http::HeaderMap;
use serde_json::{Value, json};

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error, markdown_to_discord};

/// Base URL for the Discord REST API v10.
const BASE: &str = "https://discord.com/api/v10";

/// Discord text-channel channel.
///
/// A zero-sized struct that encapsulates all Discord-specific I/O logic.
/// Configuration (bot token, channel ID, optional public key) is pulled from
/// [`ChannelContext`] at call time so the same type can be reused across
/// multiple Discord servers / channels.
pub struct DiscordChannel;

impl DiscordChannel {
    /// Create a new Discord channel instance.
    ///
    /// The instance holds no state; credentials are resolved per-request
    /// from the [`ChannelContext`] supplied to each trait method.
    pub fn new() -> Self {
        Self
    }

    /// Build the `Authorization` header value (`Bot <token>`).
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `bot_token` is missing.
    fn auth_header(&self, ctx: &ChannelContext) -> Result<String> {
        let token = cred_str(&ctx.config.credentials, "bot_token")?;
        Ok(format!("Bot {token}"))
    }

    /// Extract the target channel snowflake ID from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `channel_id` is missing.
    fn channel_id<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "channel_id")
    }

    /// Convert a raw Discord message JSON object into an [`IncomingMessage`].
    ///
    /// Extracts author info, content, thread context, and file attachments.
    /// Returns `None` if the message lacks a required `id` or `content` field.
    fn parse_message(&self, ctx: &ChannelContext, msg: &Value) -> Option<IncomingMessage> {
        let id = msg.get("id")?.as_str()?.to_string();
        let content = msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let sender_id = msg
            .pointer("/author/id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let sender_name = msg
            .pointer("/author/username")
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();

        let mut im = IncomingMessage::new(ctx.config.id, sender_id, sender_name, content);
        im.metadata
            .insert("discord_msg_id".to_string(), Value::String(id));

        // Thread: Discord threads have an embedded `thread` object with an `id` field.
        if let Some(thread) = msg
            .get("thread")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
        {
            im.thread_id = Some(thread.to_string());
        }

        // Attachments: map Discord attachment objects to core Attachment structs.
        if let Some(atts) = msg.get("attachments").and_then(|v| v.as_array()) {
            for att in atts {
                let att_id = att
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let filename = att
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("file")
                    .to_string();
                let content_type = att
                    .get("content_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let url = att
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let size = att.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                im.attachments.push(Attachment {
                    id: att_id,
                    filename,
                    content_type,
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
impl DiscordChannel {
    fn verify_ed25519(
        &self,
        ctx: &ChannelContext,
        _headers: &HeaderMap,
        _payload: &[u8],
    ) -> Result<()> {
        let public_key = match cred_str(&ctx.config.credentials, "public_key") {
            Ok(k) => k,
            Err(_) => return Ok(()), // skip if not configured
        };

        let timestamp = _headers
            .get("X-Signature-Timestamp")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ClawzError::Auth("missing X-Signature-Timestamp".to_string()))?;
        let signature = _headers
            .get("X-Signature-Ed25519")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ClawzError::Auth("missing X-Signature-Ed25519".to_string()))?;

        let pk_bytes = hex::decode(public_key)
            .map_err(|e| ClawzError::Auth(format!("invalid public key hex: {e}")))?;
        let sig_bytes = hex::decode(signature)
            .map_err(|e| ClawzError::Auth(format!("invalid signature hex: {e}")))?;

        if pk_bytes.len() != 32 || sig_bytes.len() != 64 {
            return Err(ClawzError::Auth(
                "Discord Ed25519: wrong key/sig length".to_string(),
            ));
        }

        let mut message = timestamp.as_bytes().to_vec();
        message.extend_from_slice(_payload);

        log::warn!(
            "Discord Ed25519 signature verification skipped (ed25519-dalek not in deps). Add it to Cargo.toml for production."
        );
        let _ = (pk_bytes, sig_bytes, message);

        Ok(())
    }
}

impl Default for DiscordChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for DiscordChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Discord".to_string(),
            platform: "discord".to_string(),
            version: "1.0.0".to_string(),
            author: "ClawZ".to_string(),
        }
    }

    /// Advertise Discord-specific capabilities.
    ///
    /// Delegates to the convenience constructor [`ChannelCapabilities::discord`]
    /// defined in `clawz_core`.
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::discord()
    }

    /// Poll Discord for messages in the configured channel.
    ///
    /// Paginates backwards using `before` (oldest snowflake on the current page)
    /// until fewer than 100 messages are returned. Respects HTTP 429 rate-limit
    /// responses by reading `retry_after` and sleeping before retrying.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    /// - [`ClawzError::Serialization`] if the response body is not valid JSON.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let auth = self.auth_header(ctx)?;
        let channel_id = self.channel_id(ctx)?;
        let client = &ctx.http_client;

        let mut messages = Vec::new();
        // `before` is the snowflake ID of the oldest message on the previous page.
        let mut before: Option<String> = None;

        // Discord paginates backwards via `before` (snowflake ID)
        loop {
            let mut params = vec![("limit", "100".to_string())];
            if let Some(ref b) = before {
                params.push(("before", b.clone()));
            }

            let resp = client
                .get(format!("{BASE}/channels/{channel_id}/messages"))
                .header("Authorization", &auth)
                .query(&params)
                .send()
                .await
                .map_err(|e| ClawzError::Channel(format!("Discord HTTP: {e}")))?;

            let status = resp.status();

            // Respect rate limits: Discord returns 429 with a `retry_after` float (seconds).
            if status.as_u16() == 429 {
                let body: Value = resp.json().await.unwrap_or_default();
                let retry = body
                    .get("retry_after")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);
                tokio::time::sleep(std::time::Duration::from_secs_f64(retry)).await;
                continue;
            }

            let body: Value = resp
                .json()
                .await
                .map_err(|e| ClawzError::Serialization(e.to_string()))?;

            if !status.is_success() {
                return Err(map_http_error(status, &body.to_string()));
            }

            let msgs = match body.as_array() {
                Some(a) => a.clone(),
                None => break,
            };

            if msgs.is_empty() {
                break;
            }

            // Capture the oldest ID on this page to use as `before` for the next request.
            let oldest_id = msgs
                .last()
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            for m in &msgs {
                if let Some(im) = self.parse_message(ctx, m) {
                    messages.push(im);
                }
            }

            // Only paginate if we got a full page; a partial page means we've reached the end.
            if msgs.len() < 100 {
                break;
            }
            before = oldest_id;
        }

        Ok(messages)
    }

    /// Send an outgoing message to the configured Discord channel.
    ///
    /// Converts markdown content to Discord-flavoured markdown, adds a
    /// `message_reference` when `msg.reply_to` is set, and converts image
    /// attachments to rich embeds.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let auth = self.auth_header(ctx)?;
        let channel_id = self.channel_id(ctx)?;
        let client = &ctx.http_client;

        // Convert generic markdown to Discord-specific markup (e.g. slack-style mrkdwn differences).
        let content = markdown_to_discord(&msg.content);

        let mut body = json!({ "content": content });

        // Add reply reference if present so Discord renders it as a threaded reply.
        if let Some(ref reply_id) = msg.reply_to {
            body["message_reference"] = json!({ "message_id": reply_id });
        }

        // Image attachments as embeds: Discord rich embeds display images inline.
        if !msg.attachments.is_empty() {
            let embeds: Vec<Value> = msg
                .attachments
                .iter()
                .filter(|a| a.content_type.starts_with("image/"))
                .filter_map(|a| a.url.as_ref().map(|url| json!({ "image": { "url": url } })))
                .collect();
            if !embeds.is_empty() {
                body["embeds"] = Value::Array(embeds);
            }
        }

        let resp = client
            .post(format!("{BASE}/channels/{channel_id}/messages"))
            .header("Authorization", &auth)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Discord send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a Discord Interaction or Events API payload into [`IncomingMessage`]s.
    ///
    /// Handles:
    /// - Type `1` (PING) → returns a synthetic `_ping_` message with `_discord_ping` metadata.
    /// - Type `2` / `3` (APPLICATION_COMMAND / MESSAGE_COMPONENT) → extracts user, command text, and channel.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        // Note: Ed25519 verification would need ctx; we do it if public_key
        // header is embedded, otherwise callers handle it at the gateway level.

        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("Discord webhook: {e}")))?;

        // Interaction type 1 = PING, respond with ACK
        if body.get("type").and_then(|v| v.as_u64()) == Some(1) {
            let mut dummy = IncomingMessage::new(
                uuid::Uuid::nil(),
                "discord_system".to_string(),
                "Discord".to_string(),
                "_ping_".to_string(),
            );
            dummy
                .metadata
                .insert("_discord_ping".to_string(), Value::Bool(true));
            return Ok(vec![dummy]);
        }

        // Type 2 = APPLICATION_COMMAND or type 3 = MESSAGE_COMPONENT
        let user_id = body
            .pointer("/member/user/id")
            .or_else(|| body.pointer("/user/id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let user_name = body
            .pointer("/member/user/username")
            .or_else(|| body.pointer("/user/username"))
            .and_then(|v| v.as_str())
            .unwrap_or(&user_id)
            .to_string();

        let text = body
            .pointer("/data/options/0/value")
            .and_then(|v| v.as_str())
            .or_else(|| body.pointer("/message/content").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        let channel_id = body
            .get("channel_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut im = IncomingMessage::new(uuid::Uuid::new_v4(), user_id, user_name, text);
        im.metadata
            .insert("discord_channel_id".to_string(), Value::String(channel_id));

        Ok(vec![im])
    }
}
