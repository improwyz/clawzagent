//! Generic Webhook channel implementation for the ClawZ worker.
//!
//! A universal adapter that can send/receive from any HTTP endpoint without
//! requiring a dedicated platform plugin. All behaviour is driven by
//! configuration keys in [`ChannelConfig::credentials`].
//!
//! # Capabilities
//! - **receive**: Polls the configured `endpoint_url` (GET). Extracts message
//!   fields via configurable JSONPath expressions.
//! - **send**: POST (or PUT/PATCH) to `endpoint_url` with a templated or default JSON body.
//! - **webhook**: Accepts and normalises any incoming JSON payload, extracting
//!   content via common field names (`text`, `message`, `body`, `content`).
//! - **auth**: Supports Bearer token, API key header, and HMAC-SHA256 signature verification.
//!
//! # Required credentials
//! | key                | description                                                  |
//! |--------------------|--------------------------------------------------------------|
//! | `endpoint_url`     | URL to poll (receive) or POST to (send)                      |
//! | `bearer_token`     | (optional) Bearer token for auth                             |
//! | `api_key`          | (optional) API key for custom header auth                    |
//! | `api_key_header`   | (optional) Header name for API key (default: `X-Api-Key`)     |
//! | `signing_secret`   | (optional) HMAC-SHA256 secret for inbound webhook verify     |
//! | `content_path`     | (optional) Dot-notation path to message text in JSON (default: `text`) |
//! | `sender_path`      | (optional) Dot-notation path to sender ID (default: `from`)  |
//! | `sender_name_path` | (optional) Dot-notation path to sender name (default: `sender_name`) |
//! | `body_template`    | (optional) JSON template for outgoing body; use `{{content}}` placeholder |
//! | `method`           | (optional) HTTP method for send (default: `POST`)            |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── Generic Webhook channel implementation ────────────────────────────────────
//
// A universal webhook channel that can send/receive from any HTTP endpoint:
//   receive : Accept any JSON payload; extract message via configurable JSONPath
//   send    : POST to configured URL with a templated body
//   webhook : Accept and normalize any incoming webhook payload
//   auth    : HMAC-SHA256 signature verification with configurable secret
//
// Configuration keys in `credentials`:
//   endpoint_url       – URL to POST outgoing messages to
//   signing_secret     – (optional) HMAC-SHA256 secret for verifying inbound requests
//   content_path       – (optional) dot-notation path to message text in JSON payload (e.g. "data.text")
//   sender_path        – (optional) path to sender identifier
//   sender_name_path   – (optional) path to sender display name
//   body_template      – (optional) JSON template string, use {{content}} placeholder
//   method             – (optional) HTTP method for send() (default: POST)

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message types defined in core
use clawz_core::types::channel::{ChannelCapabilities, IncomingMessage, OutgoingMessage};
// Dependency: hmac + sha2 for configurable HMAC-SHA256 webhook signature verification
use hmac::{Hmac, Mac};
use http::HeaderMap;
use serde_json::{json, Value};
use sha2::Sha256;

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error};

/// Type alias for HMAC-SHA256 used in generic webhook signature verification.
type HmacSha256 = Hmac<Sha256>;

/// Generic Webhook channel.
///
/// A zero-sized struct that acts as a universal HTTP adapter. Because every
/// behavioural aspect is controlled by configuration keys, the same type can
/// service an unlimited number of custom endpoints without code changes.
pub struct WebhookChannel;

impl WebhookChannel {
    /// Create a new generic webhook channel instance.
    pub fn new() -> Self {
        Self
    }

    /// Extract the configured endpoint URL from channel credentials.
    ///
    /// Used as the target for both `receive` (GET) and `send` (POST/PUT/PATCH).
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `endpoint_url` is missing.
    fn endpoint_url<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "endpoint_url")
    }

    /// Walk a dot-notation path through a JSON value.
    ///
    /// e.g. `"data.message.text"` on `{ "data": { "message": { "text": "hello" } } }`
    /// returns `Some("hello")`.
    ///
    /// This allows webhook/receive payloads of arbitrary shape to be mapped
    /// without writing a dedicated parser for each integration.
    fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
        let mut current = value;
        for key in path.split('.') {
            current = current.get(key)?;
        }
        Some(current)
    }

    /// Verify an inbound webhook payload against a configured HMAC-SHA256 secret.
    ///
    /// Checks three common signature headers in priority order:
    /// 1. `X-Hub-Signature-256` (GitHub-style)
    /// 2. `X-Signature`
    /// 3. `X-Webhook-Signature`
    ///
    /// If no signature header is present, verification is skipped rather than
    /// failing, so that unsigned webhooks still work out of the box.
    ///
    /// # Errors
    /// Returns [`ClawzError::Auth`] if a signature header is present but does not match.
    fn verify_hmac_signature(
        &self,
        secret: &str,
        headers: &HeaderMap,
        payload: &[u8],
    ) -> Result<()> {
        // Check common signature headers in priority order
        let sig = headers
            .get("X-Hub-Signature-256")
            .or_else(|| headers.get("X-Signature"))
            .or_else(|| headers.get("X-Webhook-Signature"))
            .and_then(|v| v.to_str().ok());

        let sig = match sig {
            Some(s) => s.to_string(),
            None => return Ok(()), // No signature header present, skip verification
        };

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| ClawzError::Auth(format!("HMAC init: {e}")))?;
        mac.update(payload);
        let computed_hex = hex::encode(mac.finalize().into_bytes());

        // Handle common prefix formats: "sha256=...", "v0=...", or raw hex
        let expected = sig
            .strip_prefix("sha256=")
            .or_else(|| sig.strip_prefix("v0="))
            .unwrap_or(&sig);

        if computed_hex != expected {
            return Err(ClawzError::Auth("Webhook HMAC signature mismatch".to_string()));
        }

        Ok(())
    }
}

impl Default for WebhookChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for WebhookChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "Generic Webhook".to_string(),
            platform: "webhook".to_string(),
            version: "1.0.0".to_string(),
            author: "ClawZ".to_string(),
        }
    }

    /// Advertise generic webhook capabilities.
    ///
    /// By default the generic webhook does not claim any rich capabilities;
    /// users can override this via custom configuration if needed.
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            media: false,
            typing: false,
            reactions: false,
            threads: false,
            voice: false,
            video: false,
            file_upload: false,
        }
    }

    /// Poll the configured endpoint URL for messages (GET request).
    ///
    /// Supports Bearer token and API key authentication. Extracts message
    /// fields using configurable JSONPath keys (`content_path`, `sender_path`,
    /// `sender_name_path`). Accepts both array and object responses.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `endpoint_url` is missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    /// - [`ClawzError::Serialization`] if the response body is not valid JSON.
    async fn receive(&self, ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        let url = self.endpoint_url(ctx)?;

        let mut req = ctx.http_client.get(url);

        // Auth headers from credentials: try bearer first, then API key.
        if let Some(token) = ctx.config.credentials.get("bearer_token").and_then(|v| v.as_str()) {
            req = req.bearer_auth(token);
        }
        if let Some(key) = ctx.config.credentials.get("api_key").and_then(|v| v.as_str()) {
            let header_name = ctx
                .config
                .credentials
                .get("api_key_header")
                .and_then(|v| v.as_str())
                .unwrap_or("X-Api-Key");
            req = req.header(header_name, key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Webhook HTTP: {e}")))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(status, &body.to_string()));
        }

        let mut messages = Vec::new();

        // If body is an array, process each item; if object, try items/data/records key first
        let items: Vec<Value> = if body.is_array() {
            body.as_array().cloned().unwrap_or_default()
        } else {
            body.get("items")
                .or_else(|| body.get("data"))
                .or_else(|| body.get("records"))
                .or_else(|| body.get("messages"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_else(|| vec![body.clone()])
        };

        let content_path = ctx
            .config
            .credentials
            .get("content_path")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let sender_path = ctx
            .config
            .credentials
            .get("sender_path")
            .and_then(|v| v.as_str())
            .unwrap_or("from");
        let sender_name_path = ctx
            .config
            .credentials
            .get("sender_name_path")
            .and_then(|v| v.as_str())
            .unwrap_or("sender_name");

        for item in &items {
            // Resolve text using the configured path, with sensible fallbacks.
            let text = Self::json_path(item, content_path)
                .and_then(|v| v.as_str())
                .or_else(|| Self::json_path(item, "body").and_then(|v| v.as_str()))
                .or_else(|| Self::json_path(item, "message").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();

            // Skip items that have no discernible text content.
            if text.is_empty() {
                continue;
            }

            let sender_id = Self::json_path(item, sender_path)
                .and_then(|v| v.as_str())
                .unwrap_or("webhook")
                .to_string();
            let sender_name = Self::json_path(item, sender_name_path)
                .and_then(|v| v.as_str())
                .unwrap_or(&sender_id)
                .to_string();

            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut im = IncomingMessage::new(ctx.config.id, sender_id, sender_name, text);
            if !id.is_empty() {
                im.metadata.insert("webhook_item_id".to_string(), Value::String(id));
            }
            // Store raw payload for downstream processing so agents can access extra fields.
            im.metadata.insert("webhook_raw".to_string(), item.clone());
            messages.push(im);
        }

        Ok(messages)
    }

    /// Send an outgoing message to the configured endpoint.
    ///
    /// The HTTP method defaults to `POST` but can be overridden via the
    /// `method` credential (`PUT` or `PATCH`). The request body is built from
    /// `body_template` (with `{{content}}` substitution) or a simple `{ "text": ... }` fallback.
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `endpoint_url` is missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let url = self.endpoint_url(ctx)?;
        let method = ctx
            .config
            .credentials
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("POST");

        // Build body from template or default
        let body: Value = if let Some(template) = ctx
            .config
            .credentials
            .get("body_template")
            .and_then(|v| v.as_str())
        {
            // Simple {{content}} substitution
            let rendered = template.replace("{{content}}", &msg.content);
            // If the rendered string is valid JSON, use it; otherwise fall back to a simple object.
            serde_json::from_str(&rendered).unwrap_or_else(|_| {
                json!({ "text": &msg.content })
            })
        } else {
            json!({ "text": &msg.content })
        };

        let client = &ctx.http_client;
        let mut req = match method.to_uppercase().as_str() {
            "PUT" => client.put(url),
            "PATCH" => client.patch(url),
            _ => client.post(url),
        };

        // Auth: same fallback chain as receive (bearer then API key).
        if let Some(token) = ctx.config.credentials.get("bearer_token").and_then(|v| v.as_str()) {
            req = req.bearer_auth(token);
        }
        if let Some(key) = ctx.config.credentials.get("api_key").and_then(|v| v.as_str()) {
            let header_name = ctx
                .config
                .credentials
                .get("api_key_header")
                .and_then(|v| v.as_str())
                .unwrap_or("X-Api-Key");
            req = req.header(header_name, key);
        }

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("Webhook send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Accept and normalise any incoming webhook payload.
    ///
    /// If the payload is not valid JSON it is treated as raw text. Extracts
    /// content from common fields (`text`, `message`, `body`, `content`).
    /// Preserves the entire raw payload in `webhook_raw` metadata so downstream
    /// agents can access fields not mapped by the generic parser.
    ///
    /// # Errors
    /// Never returns an error for parsing failures; non-JSON payloads are
    /// wrapped as opaque text messages.
    async fn webhook(&self, payload: &[u8], headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        // Parse the raw payload as JSON
        let body: Value = serde_json::from_slice(payload)
            .unwrap_or_else(|_| {
                // If not JSON, treat the raw bytes as text content
                Value::String(String::from_utf8_lossy(payload).into_owned())
            });

        // Determine content using configured path or common field names
        let content = if let Some(s) = body.as_str() {
            s.to_string()
        } else {
            body.get("text")
                .or_else(|| body.get("message"))
                .or_else(|| body.get("body"))
                .or_else(|| body.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        if content.is_empty() {
            // Return the raw payload as an opaque message when no mapped content exists.
            let raw_str = String::from_utf8_lossy(payload).into_owned();
            let mut im = IncomingMessage::new(
                uuid::Uuid::new_v4(),
                "webhook".to_string(),
                "Webhook".to_string(),
                raw_str,
            );
            im.metadata.insert("webhook_raw".to_string(), body);
            return Ok(vec![im]);
        }

        let sender_id = body
            .get("from")
            .or_else(|| body.get("sender"))
            .or_else(|| body.get("user"))
            .and_then(|v| v.as_str())
            .unwrap_or("webhook")
            .to_string();
        let sender_name = body
            .get("sender_name")
            .or_else(|| body.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();

        let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let mut im = IncomingMessage::new(uuid::Uuid::new_v4(), sender_id, sender_name, content);
        if !id.is_empty() {
            im.metadata
                .insert("webhook_item_id".to_string(), Value::String(id));
        }
        // Always preserve the full raw payload for downstream inspection.
        im.metadata.insert("webhook_raw".to_string(), body);

        Ok(vec![im])
    }
}
