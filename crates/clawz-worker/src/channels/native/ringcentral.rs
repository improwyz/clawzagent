//! RingCentral channel implementation for the ClawZ worker.
//!
//! Integrates with the [RingCentral REST API v1.0](https://developers.ringcentral.com/)
//! (Glip / Team Messaging) to read and post messages in a chat.
//!
//! # Capabilities
//! - **receive**: Polls `GET /restapi/v1.0/glip/chats/{chatId}/posts` with page-token pagination.
//! - **send**: Posts `POST /restapi/v1.0/glip/chats/{chatId}/posts`.
//! - **webhook**: Parses subscription notifications; validates via `Validation-Token` header.
//! - **auth**: OAuth2 JWT grant flow (`client_id` + `client_secret` + `jwt`) with in-memory token cache.
//!
//! # Required credentials
//! | key              | description                                      |
//! |------------------|--------------------------------------------------|
//! | `client_id`      | RingCentral app client ID                         |
//! | `client_secret`  | RingCentral app client secret                     |
//! | `jwt`            | Private JWT assertion for token exchange          |
//! | `chat_id`        | Glip chat / team ID                               |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── RingCentral channel implementation ───────────────────────────────────────
//
// Uses the RingCentral REST API v1.0 (Glip/Team Messaging):
//   receive : GET /restapi/v1.0/glip/chats/{chatId}/posts
//   send    : POST /restapi/v1.0/glip/chats/{chatId}/posts
//   webhook : Parse subscription notification; validate via validationToken header
//   auth    : OAuth2 JWT flow (client_id + client_secret + jwt)

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: base64 encoding for the HTTP Basic auth header in JWT grant
use base64::Engine as _;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message and attachment types defined in core
use clawz_core::types::channel::{Attachment, ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use serde_json::{json, Value};
// Dependency: Arc + RwLock used for the token cache because ChannelPlugin is Send + Sync
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error};

/// Base URL for the RingCentral platform API.
const BASE: &str = "https://platform.ringcentral.com";

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

/// RingCentral Glip/Team-Messaging channel.
///
/// Holds an [`Arc<RwLock<Option<TokenCache>>>`] so that multiple concurrent
/// calls (receive, send, webhook) can share a single cached access token
/// without re-authenticating on every request.
pub struct RingCentralChannel {
    /// Shared, async-safe token cache. `None` means no token has been fetched yet.
    token_cache: Arc<RwLock<Option<TokenCache>>>,
}

impl RingCentralChannel {
    /// Create a new RingCentral channel with an empty token cache.
    pub fn new() -> Self {
        Self {
            token_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Obtain a valid access token, refreshing via JWT grant if necessary.
    ///
    /// Uses a read-then-write lock pattern to minimise contention: the common
    /// case (cache hit) only acquires a read lock.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `client_id`, `client_secret`, or `jwt` are missing.
    /// - [`ClawzError::Auth`] if the token endpoint rejects the grant.
    /// - [`ClawzError::Serialization`] if the token response is malformed JSON.
    ///
    /// # JWT grant flow
    /// `POST /restapi/oauth/token` with `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`.
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

        let client_id = cred_str(&ctx.config.credentials, "client_id")?;
        let client_secret = cred_str(&ctx.config.credentials, "client_secret")?;
        let jwt = cred_str(&ctx.config.credentials, "jwt")?;

        // RingCentral requires the client credentials as a Base64-encoded Basic header.
        let creds = base64::engine::general_purpose::STANDARD
            .encode(format!("{client_id}:{client_secret}"));

        let params = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", jwt),
        ];

        let resp = ctx
            .http_client
            .post(format!("{BASE}/restapi/oauth/token"))
            .header("Authorization", format!("Basic {creds}"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Auth(format!("RingCentral token: {e}")))?;

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
            .ok_or_else(|| ClawzError::Auth("RingCentral: missing access_token".to_string()))?
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

    /// Convert a raw RingCentral post JSON object into an [`IncomingMessage`].
    ///
    /// Extracts creator info, text, and file attachments. Returns `None` if
    /// the post lacks a required `id` field.
    fn parse_post(&self, ctx: &ChannelContext, item: &Value) -> Option<IncomingMessage> {
        let id = item.get("id")?.as_str()?.to_string();
        let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let sender_id = item
            .pointer("/creator/id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let sender_name = item
            .pointer("/creator/name")
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();

        let mut im = IncomingMessage::new(ctx.config.id, sender_id, sender_name, text);
        im.metadata.insert("rc_post_id".to_string(), Value::String(id));

        // Attachments
        if let Some(atts) = item.get("attachments").and_then(|v| v.as_array()) {
            for att in atts {
                let att_id = att.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let fname = att.get("name").and_then(|v| v.as_str()).unwrap_or("file").to_string();
                let ftype = att.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                im.attachments.push(Attachment {
                    id: att_id,
                    filename: fname,
                    content_type: format!("application/{ftype}"),
                    url: None,
                    data: None,
                    size_bytes: 0,
                });
            }
        }

        Some(im)
    }
}

impl Default for RingCentralChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for RingCentralChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "RingCentral".to_string(),
            platform: "ringcentral".to_string(),
            version: "1.0.0".to_string(),
            author: "ClawZ".to_string(),
        }
    }

    /// Advertise RingCentral-specific capabilities.
    ///
    /// RingCentral Glip supports reactions, threads, voice, video, and file uploads.
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            media: true,
            typing: false,
            reactions: true,
            threads: true,
            voice: true,
            video: true,
            file_upload: true,
        }
    }

    /// Poll RingCentral for posts in the configured chat.
    ///
    /// Paginates using `pageToken` from `navigation.prevPageToken` until no
    /// further pages exist. Each page fetches up to 100 records.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials or `chat_id` are missing.
    /// - [`ClawzError::Auth`] if the JWT grant fails.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let token = self.get_token(ctx).await?;
        let chat_id = cred_str(&ctx.config.credentials, "chat_id")?;

        let mut messages = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut params = vec![("recordCount", "100".to_string())];
            if let Some(ref pt) = page_token {
                params.push(("pageToken", pt.clone()));
            }

            let resp = ctx
                .http_client
                .get(format!("{BASE}/restapi/v1.0/glip/chats/{chat_id}/posts"))
                .bearer_auth(&token)
                .query(&params)
                .send()
                .await
                .map_err(|e| ClawzError::Channel(format!("RingCentral HTTP: {e}")))?;

            let status = resp.status();
            let body: Value = resp
                .json()
                .await
                .map_err(|e| ClawzError::Serialization(e.to_string()))?;

            if !status.is_success() {
                return Err(map_http_error(status, &body.to_string()));
            }

            if let Some(records) = body.get("records").and_then(|v| v.as_array()) {
                for item in records {
                    if let Some(im) = self.parse_post(ctx, item) {
                        messages.push(im);
                    }
                }
            }

            // RingCentral uses `prevPageToken` inside `navigation` for backwards pagination.
            page_token = body
                .pointer("/navigation/prevPageToken")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            if page_token.is_none() {
                break;
            }
        }

        Ok(messages)
    }

    /// Send an outgoing post to the configured RingCentral chat.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `chat_id` is missing.
    /// - [`ClawzError::Auth`] if the JWT grant fails.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let token = self.get_token(ctx).await?;
        let chat_id = cred_str(&ctx.config.credentials, "chat_id")?;

        let body = json!({ "text": &msg.content });

        let resp = ctx
            .http_client
            .post(format!("{BASE}/restapi/v1.0/glip/chats/{chat_id}/posts"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("RingCentral send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a RingCentral webhook subscription notification.
    ///
    /// Handles the validation handshake (`Validation-Token` header) by returning
    /// a synthetic message carrying the token in metadata so the gateway layer
    /// can echo it back. Otherwise parses `body.records` into canonical messages.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        // RingCentral sends a validationToken header for subscription validation
        if let Some(token) = headers.get("Validation-Token").and_then(|v| v.to_str().ok()) {
            let mut dummy = IncomingMessage::new(
                uuid::Uuid::nil(),
                "ringcentral_system".to_string(),
                "RingCentral".to_string(),
                token.to_string(),
            );
            dummy
                .metadata
                .insert("_rc_validation_token".to_string(), Value::String(token.to_string()));
            return Ok(vec![dummy]);
        }

        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("RingCentral webhook: {e}")))?;

        let mut messages = Vec::new();

        if let Some(records) = body.get("body").and_then(|b| b.get("records")).and_then(|v| v.as_array()) {
            for rec in records {
                let msg_type = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");
                // Filter to text-based messages only; ignore Presence, Call, etc.
                if msg_type != "TextMessage" && msg_type != "Post" {
                    continue;
                }

                let text = rec.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let sender_id = rec
                    .pointer("/creator/id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let sender_name = rec
                    .pointer("/creator/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&sender_id)
                    .to_string();
                let record_id = rec.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();

                let mut im = IncomingMessage::new(uuid::Uuid::new_v4(), sender_id, sender_name, text);
                im.metadata.insert("rc_record_id".to_string(), Value::String(record_id));
                messages.push(im);
            }
        }

        Ok(messages)
    }
}
