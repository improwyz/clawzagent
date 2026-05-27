//! 3CX XAPI v1 channel for the ClawZ worker.
//!
//! Implements the [3CX Configuration REST API](https://www.3cx.com/docs/configuration-rest-api/)
//! as described in `docs/3cx-swagger.yaml` (OData under `/xapi/v1`, OAuth2 at `/connect/token`).
//!
//! # Capabilities
//! - **receive**: Polls `GET /xapi/v1/ChatMessagesHistoryView` and/or `GET /xapi/v1/ActiveCalls`.
//! - **send**: `POST /xapi/v1/Users/Pbx.MakeCall` (outbound call) or
//!   `POST /xapi/v1/ActiveCalls({Id})/Pbx.DropCall` when `metadata["action"]` is `drop_call`.
//! - **webhook**: Best-effort parse of customer push payloads (not defined in the OpenAPI spec).
//! - **auth**: OAuth2 client credentials → `POST {base_url}/connect/token`.
//!
//! # Required credentials
//! | key              | description |
//! |------------------|-------------|
//! | `base_url`       | PBX root URL (per deployment), e.g. `https://pbx.example.com` |
//! | `client_id`      | Service principal client ID (Admin → Integrations → API) |
//! | `client_secret`  | Service principal secret |
//!
//! # Optional credentials
//! | key                  | description |
//! |----------------------|-------------|
//! | `default_dn`         | Extension used as `dn` for `Pbx.MakeCall` |
//! | `default_destination`| Fallback callee when send content is empty |
//! | `queue_number`       | OData `$filter` on `QueueNumber` for chat history |
//! | `poll`               | `chat`, `active_calls`, or `chat,active_calls` (default: both) |
//! | `xapi_path`          | Override API prefix (default `/xapi/v1`) |

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
use clawz_core::types::channel::{ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::channels::plugin::{cred_str, map_http_error};

const DEFAULT_XAPI_PATH: &str = "/xapi/v1";

/// Cached OAuth2 access token.
struct TokenCache {
    token: String,
    expires_at: Instant,
}

/// 3CX XAPI v1 channel (call control + chat history polling).
pub struct ThreeCXChannel {
    token_cache: Arc<RwLock<Option<TokenCache>>>,
}

impl ThreeCXChannel {
    pub fn new() -> Self {
        Self {
            token_cache: Arc::new(RwLock::new(None)),
        }
    }

    fn xapi_path(ctx: &ChannelContext) -> &str {
        ctx.config
            .credentials
            .get("xapi_path")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_XAPI_PATH)
    }

    /// Normalize `base_url` and return the XAPI root (`…/xapi/v1`).
    fn xapi_base(ctx: &ChannelContext) -> Result<String> {
        let base = cred_str(&ctx.config.credentials, "base_url")?;
        Ok(build_xapi_base(base, Self::xapi_path(ctx)))
    }

    /// PBX host root without `/xapi/v1` (used for `/connect/token`).
    fn api_root(ctx: &ChannelContext) -> Result<String> {
        let base = cred_str(&ctx.config.credentials, "base_url")?;
        Ok(strip_xapi_suffix(
            base.trim_end_matches('/'),
            Self::xapi_path(ctx),
        ))
    }

    fn poll_modes(ctx: &ChannelContext) -> (bool, bool) {
        let mode = ctx
            .config
            .credentials
            .get("poll")
            .and_then(|v| v.as_str())
            .unwrap_or("chat,active_calls");
        let mode = mode.to_ascii_lowercase();
        let chat = mode.contains("chat");
        let calls = mode.contains("active") || mode.contains("call");
        if !chat && !calls {
            (true, true)
        } else {
            (chat, calls)
        }
    }

    async fn get_token(&self, ctx: &ChannelContext) -> Result<String> {
        {
            let cache = self.token_cache.read().await;
            if let Some(ref t) = *cache {
                if Instant::now() + Duration::from_secs(60) < t.expires_at {
                    return Ok(t.token.clone());
                }
            }
        }

        let client_id = cred_str(&ctx.config.credentials, "client_id")?;
        let client_secret = cred_str(&ctx.config.credentials, "client_secret")?;
        let root = Self::api_root(ctx)?;
        let url = format!("{root}/connect/token");

        let resp = ctx
            .http_client
            .post(&url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", client_id),
                ("client_secret", client_secret),
            ])
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("3CX token request: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(
                status,
                body.get("error_description")
                    .or_else(|| body.get("error"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&body.to_string()),
            ));
        }

        let token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ClawzError::Auth("3CX token response missing access_token".into()))?
            .to_string();

        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600);

        let expires_at = Instant::now() + Duration::from_secs(expires_in.saturating_sub(60));
        *self.token_cache.write().await = Some(TokenCache {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    async fn xapi_get(
        &self,
        ctx: &ChannelContext,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value> {
        let base = Self::xapi_base(ctx)?;
        let token = self.get_token(ctx).await?;
        let url = format!("{base}{path}");

        let mut req = ctx.http_client.get(&url).bearer_auth(&token);
        if !query.is_empty() {
            req = req.query(query);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("3CX GET {path}: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            let fallback = body.to_string();
            let detail = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(fallback.as_str());
            return Err(map_http_error(status, detail));
        }

        Ok(body)
    }

    async fn xapi_post(&self, ctx: &ChannelContext, path: &str, body: Value) -> Result<Value> {
        let base = Self::xapi_base(ctx)?;
        let token = self.get_token(ctx).await?;
        let url = format!("{base}{path}");

        let resp = ctx
            .http_client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("3CX POST {path}: {e}")))?;

        let status = resp.status();
        let resp_body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            let fallback = resp_body.to_string();
            let detail = resp_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(fallback.as_str());
            return Err(map_http_error(status, detail));
        }

        Ok(resp_body)
    }

    async fn receive_chat(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let mut query = vec![("$top", "100".into()), ("$orderby", "TimeSent desc".into())];

        if let Some(queue) = ctx
            .config
            .credentials
            .get("queue_number")
            .and_then(|v| v.as_str())
        {
            query.push(("$filter", format!("QueueNumber eq '{queue}'")));
        }

        let body = self
            .xapi_get(ctx, "/ChatMessagesHistoryView", &query)
            .await?;

        let items = body
            .get("value")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut messages = Vec::with_capacity(items.len());
        for item in &items {
            let msg_id = item
                .get("MessageId")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let text = item
                .get("Message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sender_id = item
                .get("SenderParticipantNo")
                .or_else(|| item.get("SenderParticipantName"))
                .or_else(|| item.get("SenderParticipantEmail"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let mut im = IncomingMessage::new(ctx.config.id, sender_id.clone(), sender_id, text);
            im.metadata
                .insert("threecx_message_id".into(), json!(msg_id));
            im.metadata.insert(
                "threecx_source".into(),
                Value::String("chat_history".into()),
            );
            if let Some(conv) = item.get("ConversationId") {
                im.metadata
                    .insert("threecx_conversation_id".into(), conv.clone());
            }
            messages.push(im);
        }

        Ok(messages)
    }

    async fn receive_active_calls(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let body = self
            .xapi_get(ctx, "/ActiveCalls", &[("$top", "100".into())])
            .await?;

        let items = body
            .get("value")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut messages = Vec::with_capacity(items.len());
        for item in &items {
            let id = item.get("Id").map(|v| v.to_string()).unwrap_or_default();
            let caller = item.get("Caller").and_then(|v| v.as_str()).unwrap_or("?");
            let callee = item.get("Callee").and_then(|v| v.as_str()).unwrap_or("?");
            let status = item
                .get("Status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            let text = format!("Active call {caller} → {callee} ({status})");
            let mut im = IncomingMessage::new(
                ctx.config.id,
                format!("call-{id}"),
                format!("call-{id}"),
                text,
            );
            im.metadata
                .insert("threecx_active_call_id".into(), json!(id));
            im.metadata.insert(
                "threecx_source".into(),
                Value::String("active_calls".into()),
            );
            im.metadata.insert("threecx_status".into(), json!(status));
            messages.push(im);
        }

        Ok(messages)
    }

    async fn make_call(&self, ctx: &ChannelContext, msg: &OutgoingMessage) -> Result<()> {
        let destination = msg
            .metadata
            .get("destination")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| msg.content.clone());

        let destination = if destination.is_empty() {
            ctx.config
                .credentials
                .get("default_destination")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ClawzError::Config(
                        "3CX MakeCall requires destination in message content or default_destination"
                            .into(),
                    )
                })?
        } else {
            destination
        };

        let dn = msg.metadata.get("dn").and_then(|v| v.as_str()).or_else(|| {
            ctx.config
                .credentials
                .get("default_dn")
                .and_then(|v| v.as_str())
        });

        let contact = msg.metadata.get("contact").and_then(|v| v.as_str());

        let test_call = msg
            .metadata
            .get("test_call")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut body = json!({
            "destination": destination,
            "testCall": test_call,
        });
        if let Some(d) = dn {
            body["dn"] = json!(d);
        }
        if let Some(c) = contact {
            body["contact"] = json!(c);
        }

        let result = self.xapi_post(ctx, "/Users/Pbx.MakeCall", body).await?;

        if let Some(reason) = result.get("Reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() && reason != "Success" {
                let text = result
                    .get("ReasonText")
                    .and_then(|v| v.as_str())
                    .unwrap_or(reason);
                return Err(ClawzError::Channel(format!("3CX MakeCall: {text}")));
            }
        }

        Ok(())
    }

    async fn drop_call(&self, ctx: &ChannelContext, call_id: i32) -> Result<()> {
        let path = format!("/ActiveCalls({call_id})/Pbx.DropCall");
        self.xapi_post(ctx, &path, json!({})).await?;
        Ok(())
    }
}

impl Default for ThreeCXChannel {
    fn default() -> Self {
        Self::new()
    }
}

/// Build `{root}{xapi_path}` from a configurable PBX base URL.
fn build_xapi_base(base_url: &str, xapi_path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = if xapi_path.starts_with('/') {
        xapi_path.to_string()
    } else {
        format!("/{xapi_path}")
    };
    if base.ends_with(&path) {
        base.to_string()
    } else {
        format!("{base}{path}")
    }
}

/// Strip a trailing XAPI path segment from the PBX root URL.
fn strip_xapi_suffix(base_url: &str, xapi_path: &str) -> String {
    let path = if xapi_path.starts_with('/') {
        xapi_path
    } else {
        return base_url.trim_end_matches('/').to_string();
    };
    base_url
        .trim_end_matches('/')
        .strip_suffix(path)
        .unwrap_or(base_url)
        .trim_end_matches('/')
        .to_string()
}

#[async_trait]
impl ChannelPlugin for ThreeCXChannel {
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "3CX".into(),
            platform: "3cx".into(),
            version: "2.0.0".into(),
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

    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let (poll_chat, poll_calls) = Self::poll_modes(ctx);
        let mut out = Vec::new();
        if poll_chat {
            out.extend(self.receive_chat(ctx).await?);
        }
        if poll_calls {
            out.extend(self.receive_active_calls(ctx).await?);
        }
        Ok(out)
    }

    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let action = msg
            .metadata
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("make_call");

        match action {
            "drop_call" => {
                let call_id = msg
                    .metadata
                    .get("active_call_id")
                    .or_else(|| msg.metadata.get("call_id"))
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32)
                    .ok_or_else(|| {
                        ClawzError::Config(
                            "3CX drop_call requires metadata active_call_id (integer)".into(),
                        )
                    })?;
                self.drop_call(ctx, call_id).await
            }
            _ => self.make_call(ctx, &msg).await,
        }
    }

    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("3CX webhook: {e}")))?;

        // OData-style notification or ad-hoc customer payload.
        if let Some(value) = body.get("value").and_then(|v| v.as_array()) {
            let mut messages = Vec::new();
            for item in value {
                if item.get("Message").is_some() {
                    let text = item
                        .get("Message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let sender = item
                        .get("SenderParticipantNo")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let mut im =
                        IncomingMessage::new(uuid::Uuid::new_v4(), sender.clone(), sender, text);
                    im.metadata.insert(
                        "threecx_source".into(),
                        Value::String("webhook_chat".into()),
                    );
                    messages.push(im);
                } else if item.get("Caller").is_some() || item.get("Callee").is_some() {
                    let id = item.get("Id").map(|v| v.to_string()).unwrap_or_default();
                    let caller = item.get("Caller").and_then(|v| v.as_str()).unwrap_or("?");
                    let callee = item.get("Callee").and_then(|v| v.as_str()).unwrap_or("?");
                    let status = item
                        .get("Status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let text = format!("Active call {caller} → {callee} ({status})");
                    let mut im = IncomingMessage::new(
                        uuid::Uuid::new_v4(),
                        format!("call-{id}"),
                        format!("call-{id}"),
                        text,
                    );
                    im.metadata.insert(
                        "threecx_source".into(),
                        Value::String("webhook_active_call".into()),
                    );
                    messages.push(im);
                }
            }
            return Ok(messages);
        }

        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xapi_base_appends_default_path() {
        assert_eq!(
            build_xapi_base("https://pbx.example.com", DEFAULT_XAPI_PATH),
            "https://pbx.example.com/xapi/v1"
        );
    }

    #[test]
    fn xapi_base_idempotent_when_suffix_present() {
        assert_eq!(
            build_xapi_base("https://pbx.example.com/xapi/v1", DEFAULT_XAPI_PATH),
            "https://pbx.example.com/xapi/v1"
        );
    }

    #[test]
    fn api_root_strips_xapi_suffix() {
        assert_eq!(
            strip_xapi_suffix("https://pbx.example.com/xapi/v1", DEFAULT_XAPI_PATH),
            "https://pbx.example.com"
        );
        assert_eq!(
            strip_xapi_suffix("https://pbx.example.com", DEFAULT_XAPI_PATH),
            "https://pbx.example.com"
        );
    }
}
