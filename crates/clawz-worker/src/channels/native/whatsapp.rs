//! WhatsApp Business API channel implementation for the ClawZ worker.
//!
//! Integrates with the [Meta WhatsApp Cloud API v19.0](https://developers.facebook.com/docs/whatsapp/cloud-api/)
//! to send messages and receive inbound events via webhook.
//!
//! # Capabilities
//! - **receive**: Not supported via polling; messages arrive exclusively via webhook.
//! - **send**: Posts `POST /{phone_number_id}/messages` (text or template).
//! - **webhook**: Handles `hub.challenge` verification and parses message events.
//! - **auth**: Permanent access token (WABA bearer token).
//!
//! # Required credentials
//! | key                | description                                      |
//! |--------------------|--------------------------------------------------|
//! | `access_token`     | Meta WABA permanent access token                   |
//! | `phone_number_id`  | WhatsApp Business account phone number ID          |
//! | `default_recipient`| (optional) Fallback recipient for outgoing messages |
//!
//! # Role in architecture
//! This is a native channel in the worker execution layer. It implements the
//! [`ChannelPlugin`] trait defined in `clawz_core` and is registered by
//! [`crate::channels::native::register_all`].

// ── WhatsApp Business API channel implementation ──────────────────────────────
//
// Uses the Meta WhatsApp Cloud API v19.0:
//   receive : Messages arrive via webhook only (no polling endpoint)
//   send    : POST /{phone_number_id}/messages (text or template)
//   webhook : hub.challenge verification + parse message events
//   auth    : Permanent access token (WABA bearer token)

// Dependency: async_trait enables async methods in traits for the worker runtime
use async_trait::async_trait;
// Dependency: core error types shared across gateway → worker → core tiers
use clawz_core::error::{ClawzError, Result};
// Dependency: ChannelPlugin trait lives in the shared core crate
use clawz_core::traits::{ChannelContext, ChannelMetadata, ChannelPlugin};
// Dependency: canonical message and attachment types defined in core
use clawz_core::types::channel::{Attachment, ChannelCapabilities, IncomingMessage, OutgoingMessage};
use http::HeaderMap;
use serde_json::{json, Value};

// Dependency: helper utilities from the parent plugin module (worker-internal)
use crate::channels::plugin::{cred_str, map_http_error};

/// Base URL for the Meta WhatsApp Cloud API v19.0.
const BASE: &str = "https://graph.facebook.com/v19.0";

/// WhatsApp Business channel.
///
/// A zero-sized struct that encapsulates all WhatsApp Cloud API I/O logic.
/// Configuration (access token, phone number ID) is pulled from
/// [`ChannelContext`] at call time so the same type can be reused across
/// multiple WABA accounts.
pub struct WhatsAppChannel;

impl WhatsAppChannel {
    /// Create a new WhatsApp channel instance.
    ///
    /// The instance holds no state; credentials are resolved per-request
    /// from the [`ChannelContext`] supplied to each trait method.
    pub fn new() -> Self {
        Self
    }

    /// Extract the Meta WABA access token from channel credentials.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `access_token` is missing.
    fn token<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "access_token")
    }

    /// Extract the WhatsApp phone number ID from channel credentials.
    ///
    /// This ID is used as the path segment in send requests.
    ///
    /// # Errors
    /// Returns [`ClawzError::Config`] if `phone_number_id` is missing.
    fn phone_number_id<'a>(&self, ctx: &'a ChannelContext) -> Result<&'a str> {
        cred_str(&ctx.config.credentials, "phone_number_id")
    }

    /// Convert a raw WhatsApp message JSON object into an [`IncomingMessage`].
    ///
    /// Handles multiple message types: `text`, `image`, `video`, `audio`,
    /// `document`, `sticker`, and `location`. Media messages are represented as
    /// [`Attachment`]s with empty URLs (media must be fetched separately via
    /// the media ID endpoint). Returns `None` if the message lacks `id`, `type`,
    /// or `from` fields.
    #[allow(dead_code)]
    fn parse_message(&self, ctx: &ChannelContext, msg: &Value, contact: Option<&Value>) -> Option<IncomingMessage> {
        let msg_id = msg.get("id")?.as_str()?.to_string();
        let msg_type = msg.get("type")?.as_str()?;
        let from = msg.get("from")?.as_str()?.to_string();

        let sender_name = contact
            .and_then(|c| c.get("profile"))
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&from)
            .to_string();

        let (text, attachments) = match msg_type {
            "text" => {
                let t = msg
                    .pointer("/text/body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (t, Vec::new())
            }
            "image" | "video" | "audio" | "document" | "sticker" => {
                let media = msg.get(msg_type).cloned().unwrap_or_default();
                let caption = media
                    .get("caption")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let media_id = media
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mime = media
                    .get("mime_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let filename = media
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or(msg_type)
                    .to_string();
                let att = Attachment {
                    id: media_id,
                    filename,
                    content_type: mime,
                    url: None, // Fetched via /{media_id} endpoint with the access token
                    data: None,
                    size_bytes: 0,
                };
                (caption, vec![att])
            }
            "location" => {
                let loc = msg.get("location").cloned().unwrap_or_default();
                let lat = loc.get("latitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let lon = loc.get("longitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let name = loc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("location")
                    .to_string();
                (format!("[location: {name} ({lat},{lon})]"), Vec::new())
            }
            // Fallback for unsupported types (contacts, interactive, etc.)
            _ => (format!("[{msg_type}]"), Vec::new()),
        };

        let mut im = IncomingMessage::new(ctx.config.id, from.clone(), sender_name, text);
        im.attachments = attachments;
        im.metadata.insert("wa_msg_id".into(), Value::String(msg_id));
        im.metadata.insert("wa_from".into(), Value::String(from));

        Some(im)
    }
}

impl Default for WhatsAppChannel {
    /// Convenience shorthand for [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for WhatsAppChannel {
    /// Return static metadata describing this plugin.
    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "WhatsApp Business".into(),
            platform: "whatsapp".into(),
            version: "1.0.0".into(),
            author: "ClawZ".into(),
        }
    }

    /// Advertise WhatsApp-specific capabilities.
    ///
    /// WhatsApp supports media, reactions, and file uploads. Voice and video
    /// calls are not supported via the Cloud API message endpoint.
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            media: true,
            typing: false,
            reactions: true,
            threads: false,
            voice: false,
            video: false,
            file_upload: true,
        }
    }

    /// WhatsApp messages arrive via webhook only; polling is not supported by the Cloud API.
    ///
    /// Always returns an empty vector. Inbound messages are handled in [`Self::webhook`].
    async fn receive(&self, _ctx: &ChannelContext) -> Result<Vec<IncomingMessage>> {
        Ok(Vec::new())
    }

    /// Send an outgoing message via the WhatsApp Cloud API.
    ///
    /// The recipient phone number is resolved in this priority:
    /// 1. `msg.metadata["to"]`
    /// 2. `ctx.config.credentials["default_recipient"]`
    ///
    /// If `msg.metadata["template_name"]` is present, a template message is
    /// sent instead of free-form text (required for outbound messaging to users
    /// who have not interacted in the last 24 hours).
    ///
    /// # Errors
    /// - [`ClawzError::Config`] if `access_token`, `phone_number_id`, or recipient are missing.
    /// - [`ClawzError::Channel`] on HTTP failure.
    async fn send(&self, ctx: &ChannelContext, msg: OutgoingMessage) -> Result<()> {
        let token = self.token(ctx)?;
        let phone_number_id = self.phone_number_id(ctx)?;

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
            .ok_or_else(|| ClawzError::Config("WhatsApp: missing 'to' phone number".into()))?
            .to_string();

        // Template messages bypass the 24-hour session window but require pre-approval.
        let body = if let Some(template) =
            msg.metadata.get("template_name").and_then(|v| v.as_str())
        {
            let lang = msg
                .metadata
                .get("template_language")
                .and_then(|v| v.as_str())
                .unwrap_or("en_US");
            json!({
                "messaging_product": "whatsapp",
                "to": to,
                "type": "template",
                "template": {
                    "name": template,
                    "language": { "code": lang }
                }
            })
        } else {
            json!({
                "messaging_product": "whatsapp",
                "to": to,
                "type": "text",
                "text": { "body": &msg.content }
            })
        };

        let resp = ctx
            .http_client
            .post(format!("{BASE}/{phone_number_id}/messages"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClawzError::Channel(format!("WhatsApp send: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status, &text));
        }

        Ok(())
    }

    /// Parse a WhatsApp webhook payload into [`IncomingMessage`]s.
    ///
    /// Handles two distinct payload shapes:
    /// 1. **Hub challenge verification** (`hub.challenge`) → returns a synthetic
    ///    message with `_wa_challenge` metadata so the gateway can echo it.
    /// 2. **Message events** (`entry[].changes[].value.messages[]`) → parses
    ///    each message, looks up the matching contact profile for the sender
    ///    name, and builds canonical messages with `wa_msg_id` / `wa_from` metadata.
    ///
    /// # Errors
    /// - [`ClawzError::Serialization`] if the payload is not valid JSON.
    async fn webhook(&self, payload: &[u8], _headers: &HeaderMap) -> Result<Vec<IncomingMessage>> {
        let body: Value = serde_json::from_slice(payload)
            .map_err(|e| ClawzError::Serialization(format!("WhatsApp webhook: {e}")))?;

        // URL verification challenge (hub.challenge is forwarded as JSON by the gateway)
        if let Some(challenge) = body.get("hub.challenge").and_then(|v| v.as_str()) {
            let verify_token = body
                .get("hub.verify_token")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut dummy = IncomingMessage::new(
                uuid::Uuid::nil(),
                "whatsapp_system".to_string(),
                "WhatsApp".to_string(),
                challenge.to_string(),
            );
            dummy
                .metadata
                .insert("_wa_challenge".to_string(), Value::String(challenge.to_string()));
            dummy
                .metadata
                .insert("_wa_verify_token".to_string(), Value::String(verify_token.to_string()));
            return Ok(vec![dummy]);
        }

        let mut messages = Vec::new();

        if let Some(entries) = body.get("entry").and_then(|v| v.as_array()) {
            for entry in entries {
                if let Some(changes) = entry.get("changes").and_then(|v| v.as_array()) {
                    for change in changes {
                        // Skip non-message change types (e.g. account updates, template events).
                        if change.get("field").and_then(|v| v.as_str()) != Some("messages") {
                            continue;
                        }

                        let value = change.get("value").cloned().unwrap_or_default();
                        let contacts = value.get("contacts").and_then(|v| v.as_array()).cloned();

                        if let Some(msgs_arr) = value.get("messages").and_then(|v| v.as_array()) {
                            for m in msgs_arr {
                                let from = m.get("from").and_then(|v| v.as_str()).unwrap_or("unknown");
                                // Look up the contact profile that matches the `from` phone number.
                                let contact = contacts.as_ref().and_then(|cs| {
                                    cs.iter().find(|c| {
                                        c.get("wa_id").and_then(|v| v.as_str()) == Some(from)
                                    })
                                });

                                // Parse message inline (webhook has no ctx)
                                let msg_type = m.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                                let text = match msg_type {
                                    "text" => m.pointer("/text/body").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    "location" => {
                                        let lat = m.pointer("/location/latitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                        let lon = m.pointer("/location/longitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                        format!("[location: ({lat},{lon})]")
                                    }
                                    // Fallback for unsupported webhook types
                                    t => format!("[{t}]"),
                                };
                                let msg_id = m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let sender_name = contact
                                    .and_then(|c| c.get("profile"))
                                    .and_then(|p| p.get("name"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(from)
                                    .to_string();

                                let mut im = IncomingMessage::new(
                                    uuid::Uuid::new_v4(),
                                    from.to_string(),
                                    sender_name,
                                    text,
                                );
                                im.metadata.insert("wa_msg_id".to_string(), Value::String(msg_id));
                                im.metadata.insert("wa_from".to_string(), Value::String(from.to_string()));
                                messages.push(im);
                            }
                        }
                    }
                }
            }
        }

        Ok(messages)
    }
}
