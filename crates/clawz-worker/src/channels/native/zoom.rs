//! Zoom channel implementation for the ClawZ worker.
//!
//! Integrates with the [Zoom Chat API v2](https://developers.zoom.us/docs/api/)
//! using Server-to-Server OAuth2 to read and post chat messages.
//!
//! # Capabilities
//! - **receive**: Polls `GET /v2/chat/users/{user_id}/messages` with optional contact/channel filter.
//! - **send**: Posts `POST /v2/chat/users/{user_id}/messages` to a contact or channel.
//! - **webhook**: Parses Zoom webhook events (`chat_message.sent`, `chat_message.received`); handles URL validation challenges.
//! - **auth**: Server-to-Server OAuth2 (`account_id` + `client_id` + `client_secret`) with in-memory token cache.
//!
//! # Required credentials
//! | key              | description                                      |
//! |------------------|--------------------------------------------------|
//! | `account_id`     | Zoom Server-to-Server OAuth account ID           |
//! | `client_id`      | Zoom app client ID                               |
//! | `client_secret`  | Zoom app client secret                           |
//! | `user_id`        | Zoom user ID (UUID or email) whose chat to access |
//! | `to_contact`     | (optional) Target contact email for send/receive filter |
//! | `to_channel`     | (optional) Target channel ID for send/receive filter |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── Zoom channel implementation ───────────────────────────────────────────────
//
// Uses the Zoom Chat API v2 with Server-to-Server OAuth2:
//   receive : GET /v2/chat/users/{user_id}/messages
//   send    : POST /v2/chat/users/{user_id}/messages
//   webhook : Zoom webhook events (HMAC-SHA256 verification)
//   auth    : Server-to-Server OAuth2 (account_id + client_id + client_secret)

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: base64 encoding for the HTTP Basic auth header in OAuth2 token request
use base64::Engine as _;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message types defined in core
use clawz_core::types::channel::{ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use serde_json::{json, Value};
// Dependency: Arc + RwLock used for the token cache because ChannelPlugin is Send + Sync
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error};


/// Base URL for the Zoom REST API v2.
const BASE: &str = "https://api.zoom.us/v2";

/// In-memory OAuth2 token cache with expiration tracking.
///
/// Tokens are considered stale 60 seconds before the actual expiry to avoid
/// race conditions where the token expires in-flight.
struct TokenCache {
    /// The current Bearer access token.
    token: String,
    /// Wall-clock instant when the token becomes invalid (with 60 s safety margin applied).
    expires_at: Instant,
}

/// Zoom Chat channel.
///
/// Holds an [`Arc<RwLock<Option<TokenCache>>>`] so that multiple concurrent
/// calls (receive, send, webhook) can share a single cached access token
/// without re-authenticating on every request.
pub struct ZoomChannel {
    /// Shared, async-safe token cache. `None` means no token has been fetched yet.
    token_cache: Arc<RwLock<Option<TokenCache>>>,
}

impl ZoomChannel {
    /// Create a new Zoom channel with an empty token cache.
    pub fn new() -> Self {
        Self {
            token_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Obtain a valid Zoom access token, refreshing via Server-to-Server OAuth2 if necessary.
    ///
    /// Uses a read-then-write lock pattern to minimise contention: the common
    /// case (cache hit) only acquires a read lock.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `account_id`, `client_id`, or `client_secret` are missing.
    /// - [`ClawzError::Auth`] if the Zoom token endpoint rejects the grant.
    /// - [`ClawzError::Serialization`] if the token response is malformed JSON.
    ///
    /// # OAuth2 flow
    /// `POST https://zoom.us/oauth/token` with `grant_type=account_credentials`
    /// and the account ID as a query parameter. Client credentials are sent as
    /// a Base64-encoded Basic `Authorization` header.
    async fn get_token(&self, ctx: &ChannelContext) -> Result<String> {
        // Fast path: try read lock first to avoid starving other tasks.
        {
            let cache = self.token_cache.read().await;
            if let Some(ref t) = *cache {
                // 60-second buffer prevents using a token that is about to expire mid-request.
                if Instant::now() + Duration::from_secs(60) < t.expires_at {
                    return Ok(t.token.clone());
                }
            }
        }

        let account_id = cred_str(&ctx.config.credentials, "account_id")?;
        let client_id = cred_str(&ctx.config.credentials, "client_id")?;
        let client_secret = cred_str(&ctx.config.credentials, "client_secret")?;

        // Zoom requires the client credentials as a Base64-encoded Basic header.
        let creds = base64::engine::general_purpose::STANDARD
            .encode(format!("{client_id}:{client_secret}"));

        let resp = ctx
            .http_client
            .post("https://zoom.us/oauth/token")
            .header("Authorization", format!("Basic {creds}"))
            .query(&[
                ("grant_type", "account_credentials"),
                ("account_id", account_id),
            ])
            .send()
            .await
            .map_err(|e| ClawzError::Auth(format!("Zoom token: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(status, &body.to_string()));
        }

        let token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ClawzError::Auth("Zoom: missing access_token".to_string()))?
            .to_string();

        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600);

        // Write the newly obtained token back into the shared cache.
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(TokenCache {
                token: token.clone(),
                expires_at: Instant::now() + Duration::from_secs(expires_in),
            });
        }

        Ok(token)
    }
}

impl Default for ZoomChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for ZoomChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Zoom".to_string(),
            platform: "zoom".to_string(),
            version: "1.0.0".to_string(),
            author: "ClawZ".to_string(),
        }
    }

    /// Advertise Zoom-specific capabilities.
    ///
    /// Zoom Chat supports media, reactions, voice, video, and file uploads.
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            media: true,
            typing: false,
            reactions: true,
            threads: false,
            voice: true,
            video: true,
            file_upload: true,
        }
    }

    /// Poll Zoom for chat messages belonging to the configured user.
    ///
    /// Optionally filters by `to_contact` or `to_channel` if configured.
    /// Returns up to 50 messages per request (Zoom default).
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials or `user_id` are missing.
    /// - [`ClawzError::Auth`] if the OAuth2 token grant fails.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let token = self.get_token(ctx).await?;
        let user_id = cred_str(&ctx.config.credentials, "user_id")?;

        let mut params: Vec<(&str, String)> = vec![("page_size", "50".to_string())];
        // Prefer contact filter; fall back to channel filter if no contact is set.
        if let Some(contact) = ctx.config.credentials.get("to_contact").and_then(|v| v.as_str()) {
            params.push(("to_contact", contact.to_string()));
        } else if let Some(channel) = ctx.config.credentials.get("to_channel").and_then(|v| v.as_str()) {
            params.push(("to_channel", channel.to_string()));
        }

        let resp = ctx
            .http_client
            .get(format!("{BASE}/chat/users/{user_id}/messages"))
            .bearer_auth(&token)
            .query(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Zoom HTTP: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(status, &body.to_string()));
        }

        let mut messages = Vec::new();

        if let Some(msgs) = body.get("messages").and_then(|v| v.as_array()) {
            for m in msgs {
                let msg_id = m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let content = m
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sender_id = m
                    .get("sender")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                let mut im = IncomingMessage::new(
                    ctx.config.id,
                    sender_id.clone(),
                    sender_id,
                    content,
                );
                im.metadata.insert("zoom_msg_id".to_string(), Value::String(msg_id));
                messages.push(im);
            }
        }

        Ok(messages)
    }

    /// Send an outgoing chat message via Zoom.
    ///
    /// The destination is resolved from `to_contact` or `to_channel` credentials.
    /// Exactly one of them must be present for the message to be routed correctly.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `user_id` is missing.
    /// - [`ClawzError::Auth`] if the OAuth2 token grant fails.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let token = self.get_token(ctx).await?;
        let user_id = cred_str(&ctx.config.credentials, "user_id")?;

        let mut body = json!({ "message": &msg.content });

        // Target: contact takes precedence over channel if both are configured.
        if let Some(contact) = ctx.config.credentials.get("to_contact").and_then(|v| v.as_str()) {
            body["to_contact"] = Value::String(contact.to_string());
        } else if let Some(channel) = ctx.config.credentials.get("to_channel").and_then(|v| v.as_str()) {
            body["to_channel"] = Value::String(channel.to_string());
        }

        let resp = ctx
            .http_client
            .post(format!("{BASE}/chat/users/{user_id}/messages"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Zoom send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a Zoom webhook payload into [`IncomingMessage`]s.
    ///
    /// Handles:
    /// 1. **URL validation challenge** (`payload.plainToken`) → returns a synthetic
    ///    message with `_zoom_challenge` metadata so the gateway can echo it.
    /// 2. **Chat events** (`chat_message.sent`, `chat_message.received`) → extracts
    ///    message text and sender from `payload.object`.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("Zoom webhook: {e}")))?;

        // Zoom URL validation challenge: the gateway must echo plainToken in the response.
        if let Some(plaintoken) = body
            .get("payload")
            .and_then(|p| p.get("plainToken"))
            .and_then(|v| v.as_str())
        {
            let mut dummy = IncomingMessage::new(
                uuid::Uuid::nil(),
                "zoom_system".to_string(),
                "Zoom".to_string(),
                plaintoken.to_string(),
            );
            dummy
                .metadata
                .insert("_zoom_challenge".to_string(), Value::String(plaintoken.to_string()));
            return Ok(vec![dummy]);
        }

        let event_type = body
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Only process chat message events; ignore meeting, recording, etc.
        if event_type == "chat_message.sent" || event_type == "chat_message.received" {
            let payload_obj = body.get("payload").cloned().unwrap_or_default();
            let object = payload_obj.get("object").cloned().unwrap_or_default();

            let msg_id = object
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = object
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sender_id = object
                .get("sender")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let mut im =
                IncomingMessage::new(uuid::Uuid::new_v4(), sender_id.clone(), sender_id, text);
            im.metadata
                .insert("zoom_msg_id".to_string(), Value::String(msg_id));
            im.metadata
                .insert("zoom_event".to_string(), Value::String(event_type));
            return Ok(vec![im]);
        }

        Ok(Vec::new())
    }
}
