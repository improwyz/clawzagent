//! Microsoft Teams channel implementation for the ClawZ worker.
//!
//! Integrates with the [Microsoft Graph API](https://learn.microsoft.com/en-us/graph/api/resources/teams-api-overview)
//! to read and post messages in a Teams channel.
//!
//! # Capabilities
//! - **receive**: Polls `GET /teams/{team_id}/channels/{channel_id}/messages` with `@odata.nextLink` pagination.
//! - **send**: Posts `POST /teams/{team_id}/channels/{channel_id}/messages` (HTML body).
//! - **webhook**: Parses Bot Framework Activity payloads.
//! - **auth**: Azure AD client-credentials OAuth2 token (cached in-memory with 60 s safety margin).
//!
//! # Required credentials
//! | key              | description                                      |
//! |------------------|--------------------------------------------------|
//! | `tenant_id`      | Azure AD tenant ID                               |
//! | `client_id`      | Azure AD application (client) ID                 |
//! | `client_secret`  | Azure AD application client secret               |
//! | `team_id`        | Microsoft Teams group ID                         |
//! | `channel_id`     | Target channel ID within the team                |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── Microsoft Teams channel implementation ────────────────────────────────────
//
// Uses the Microsoft Graph API:
//   receive : GET /teams/{team_id}/channels/{channel_id}/messages
//   send    : POST /teams/{team_id}/channels/{channel_id}/messages
//   webhook : Bot Framework Activity payload
//   auth    : Azure AD client-credentials OAuth2 token (cached)

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message types defined in core
use clawz_core::types::channel::{ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use serde_json::{Value, json};
// Dependency: Arc + RwLock used for the token cache because ChannelPlugin is Send + Sync
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error};

/// Base URL for the Microsoft Graph API v1.0.
const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

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

/// Microsoft Teams channel.
///
/// Holds an [`Arc<RwLock<Option<TokenCache>>>`] so that multiple concurrent
/// calls (receive, send, webhook) can share a single cached Graph access token
/// without re-authenticating on every request.
pub struct TeamsChannel {
    /// Shared, async-safe token cache. `None` means no token has been fetched yet.
    token_cache: Arc<RwLock<Option<TokenCache>>>,
}

impl TeamsChannel {
    /// Create a new Teams channel with an empty token cache.
    pub fn new() -> Self {
        Self {
            token_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Obtain a valid Microsoft Graph access token, refreshing via client-credentials grant if necessary.
    ///
    /// Uses a read-then-write lock pattern to minimise contention: the common
    /// case (cache hit) only acquires a read lock.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `tenant_id`, `client_id`, or `client_secret` are missing.
    /// - [`ClawzError::Auth`] if the Azure AD token endpoint rejects the grant.
    /// - [`ClawzError::Serialization`] if the token response is malformed JSON.
    ///
    /// # OAuth2 flow
    /// `POST https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token`
    /// with `grant_type=client_credentials` and scope `https://graph.microsoft.com/.default`.
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

        let tenant_id = cred_str(&ctx.config.credentials, "tenant_id")?;
        let client_id = cred_str(&ctx.config.credentials, "client_id")?;
        let client_secret = cred_str(&ctx.config.credentials, "client_secret")?;

        let url = format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token");
        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            // Request the default set of Graph permissions granted to the app.
            ("scope", "https://graph.microsoft.com/.default"),
        ];

        let resp = ctx
            .http_client
            .post(&url)
            .form(&params)
            .send()
            .await
            .map_err(|e| ClawzError::Auth(format!("Teams token: {e}")))?;

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
            .ok_or_else(|| ClawzError::Auth("Teams: missing access_token".into()))?
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

    /// Convert a raw Graph message item into an [`IncomingMessage`].
    ///
    /// Strips HTML tags from the message body (Teams returns HTML content)
    /// and extracts sender info from `/from/user`. Returns `None` if the item
    /// lacks a required `id` field.
    fn parse_item(&self, ctx: &ChannelContext, item: &Value) -> Option<IncomingMessage> {
        let id = item.get("id")?.as_str()?.to_string();
        let raw_content = item
            .pointer("/body/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Teams returns HTML content; we strip tags for plain-text downstream processing.
        let content = html_strip(&raw_content);

        let sender_id = item
            .pointer("/from/user/id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let sender_name = item
            .pointer("/from/user/displayName")
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();

        let mut im = IncomingMessage::new(ctx.config.id, sender_id, sender_name, content);
        im.metadata.insert("teams_msg_id".into(), Value::String(id));
        Some(im)
    }
}

impl Default for TeamsChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Very fast, allocation-light HTML tag stripper.
///
/// Iterates character-by-character, skipping everything between `<` and `>`.
/// Not a full HTML parser — sufficient for Teams message bodies that only
/// contain simple formatting tags (`<p>`, `<b>`, etc.).
fn html_strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[async_trait]
impl ChannelPlugin for TeamsChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Microsoft Teams".into(),
            platform: "teams".into(),
            version: "1.0.0".into(),
            author: "ClawZ".into(),
        }
    }

    /// Advertise Teams-specific capabilities.
    ///
    /// Teams supports rich media, reactions, threads, voice, video, and file uploads.
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

    /// Poll Microsoft Graph for messages in the configured Teams channel.
    ///
    /// Paginates via `@odata.nextLink` until no further pages exist.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials or IDs are missing.
    /// - [`ClawzError::Auth`] if the Azure AD token grant fails.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let token = self.get_token(ctx).await?;
        let team_id = cred_str(&ctx.config.credentials, "team_id")?;
        let channel_id = cred_str(&ctx.config.credentials, "channel_id")?;

        let base_url = format!("{GRAPH_BASE}/teams/{team_id}/channels/{channel_id}/messages");

        let mut messages = Vec::new();
        let mut next_url: Option<String> = Some(base_url);

        while let Some(url) = next_url {
            let resp = ctx
                .http_client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| ClawzError::Channel(format!("Teams HTTP: {e}")))?;

            let status = resp.status();
            let body: Value = resp
                .json()
                .await
                .map_err(|e| ClawzError::Serialization(e.to_string()))?;

            if !status.is_success() {
                return Err(map_http_error(status, &body.to_string()));
            }

            if let Some(items) = body.get("value").and_then(|v| v.as_array()) {
                for item in items {
                    if let Some(im) = self.parse_item(ctx, item) {
                        messages.push(im);
                    }
                }
            }

            // Graph uses `@odata.nextLink` for forward pagination.
            next_url = body
                .get("@odata.nextLink")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

        Ok(messages)
    }

    /// Send an outgoing message to the configured Teams channel.
    ///
    /// Wraps the content in a simple `<p>` tag because Graph expects HTML
    /// when `contentType` is `"html"`.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if credentials or IDs are missing.
    /// - [`ClawzError::Auth`] if the Azure AD token grant fails.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let token = self.get_token(ctx).await?;
        let team_id = cred_str(&ctx.config.credentials, "team_id")?;
        let channel_id = cred_str(&ctx.config.credentials, "channel_id")?;

        let url = format!("{GRAPH_BASE}/teams/{team_id}/channels/{channel_id}/messages");

        let body = json!({
            "body": {
                "contentType": "html",
                "content": format!("<p>{}</p>", &msg.content)
            }
        });

        let resp = ctx
            .http_client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Teams send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a Bot Framework Activity payload into [`IncomingMessage`]s.
    ///
    /// Only processes activities of `type == "message"`. Extracts sender
    /// identity from `/from` and the channel reference from `/channelData/channel/id`.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let activity: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("Teams webhook: {e}")))?;

        // Ignore non-message activities (e.g. conversationUpdate, typing).
        if activity.get("type").and_then(|v| v.as_str()) != Some("message") {
            return Ok(Vec::new());
        }

        let text = activity
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let sender_id = activity
            .pointer("/from/id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let sender_name = activity
            .pointer("/from/name")
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();

        // channelData holds Teams-specific context (tenant, channel, team).
        let channel_ref = activity
            .pointer("/channelData/channel/id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut im = IncomingMessage::new(uuid::Uuid::new_v4(), sender_id, sender_name, text);
        im.metadata
            .insert("teams_channel_ref".into(), Value::String(channel_ref));

        Ok(vec![im])
    }
}
