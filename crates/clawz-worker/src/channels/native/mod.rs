//! Native channel implementations for the ClawZ worker execution layer.
//!
//! This module bundles concrete implementations of the [`ChannelPlugin`] trait
//! for popular communication platforms (including Twilio and Google Voice). Each submodule encapsulates the
//! platform-specific HTTP API, authentication flow, pagination strategy, and
//! webhook normalization logic required to turn external messages into the
//! worker's canonical [`IncomingMessage`] / [`OutgoingMessage`] types.
//!
//! # Role in the 3-tier architecture
//!
//! - **gateway** (API layer) receives raw HTTP webhooks and forwards them
//!   to the worker via the runtime module.
//! - **worker** (execution layer) — this module — performs the actual
//!   platform I/O (poll, send, webhook parse) inside [`ChannelPlugin`] actors.
//! - **core** (shared types/traits) defines [`ChannelPlugin`],
//!   [`ChannelContext`], [`ChannelRegistry`], and portable message types.
//!
//! # Adding a new channel
//!
//! 1. Create a new submodule file (e.g. `signal.rs`).
//! 2. Implement [`ChannelPlugin`] for a zero-sized or stateful struct.
//! 3. Re-export it here and register it in [`register_all`].
//!
//! # Key dependencies
//!
//! - `clawz_core::traits::{ChannelPlugin, ChannelContext, ChannelMetadata}`
//! - `clawz_core::types::channel::{ChannelConfig, ChannelCapabilities, IncomingMessage, OutgoingMessage}`
//! - `crate::channels::registry::ChannelRegistry`
//!
// ── Native channel implementations ───────────────────────────────────────────

pub mod dialpad;
pub mod discord;
pub mod google_voice;
pub mod ringcentral;
pub mod slack;
pub mod teams;
pub mod threecx;
pub mod twilio;
pub mod webex;
pub mod webhook;
pub mod whatsapp;
pub mod zoom;

// Dependency: Re-export each channel struct so consumers only need `native::*`
pub use dialpad::DialpadChannel;
pub use discord::DiscordChannel;
pub use google_voice::GoogleVoiceChannel;
pub use ringcentral::RingCentralChannel;
pub use slack::SlackChannel;
pub use teams::TeamsChannel;
pub use threecx::ThreeCXChannel;
pub use twilio::TwilioChannel;
pub use webex::WebexChannel;
pub use webhook::WebhookChannel;
pub use whatsapp::WhatsAppChannel;
pub use zoom::ZoomChannel;

// Dependency: ChannelRegistry lives in the sibling `registry` module
use crate::channels::registry::ChannelRegistry;
// Dependency: ChannelConfig is defined in the shared `core` crate
use clawz_core::types::channel::ChannelConfig;

/// Register all native channel implementations into the given registry.
///
/// Each channel is constructed with a default [`ChannelConfig`] keyed by
/// platform name.  Callers may add additional instances (e.g. a second Slack
/// workspace) by calling [`ChannelRegistry::register`] directly.
///
/// # Panics
///
/// Never panics — each channel constructor is infallible.
pub fn register_all(registry: &mut ChannelRegistry) {
    /// Helper macro to reduce boilerplate when registering a native channel.
    ///
    /// Expands to:
    /// 1. Build a [`ChannelConfig`] with the given platform string.
    /// 2. Construct the channel via `::new()`.
    /// 3. Register both into the [`ChannelRegistry`].
    macro_rules! native {
        ($platform:expr, $channel:expr) => {{
            let cfg = ChannelConfig::new($platform, serde_json::Value::Object(Default::default()));
            registry.register(Box::new($channel), &cfg);
        }};
    }

    native!("slack", SlackChannel::new());
    native!("teams", TeamsChannel::new());
    native!("discord", DiscordChannel::new());
    native!("zoom", ZoomChannel::new());
    native!("webex", WebexChannel::new());
    native!("whatsapp", WhatsAppChannel::new());
    native!("3cx", ThreeCXChannel::new());
    native!("ringcentral", RingCentralChannel::new());
    native!("dialpad", DialpadChannel::new());
    native!("twilio", TwilioChannel::new());
    native!("google_voice", GoogleVoiceChannel::new());
    native!("webhook", WebhookChannel::new());
}
