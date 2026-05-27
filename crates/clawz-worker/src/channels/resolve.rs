//! Resolve native channel plugins by platform name.

use clawz_core::traits::ChannelPlugin;

use crate::channels::native::{
    dialpad::DialpadChannel, discord::DiscordChannel, google_voice::GoogleVoiceChannel,
    ringcentral::RingCentralChannel, slack::SlackChannel, teams::TeamsChannel,
    threecx::ThreeCXChannel, twilio::TwilioChannel, webex::WebexChannel, webhook::WebhookChannel,
    whatsapp::WhatsAppChannel, zoom::ZoomChannel,
};

/// Construct a channel plugin for the given platform id.
pub fn plugin_for_platform(platform: &str) -> Option<Box<dyn ChannelPlugin>> {
    match platform.to_ascii_lowercase().as_str() {
        "slack" => Some(Box::new(SlackChannel::new())),
        "teams" => Some(Box::new(TeamsChannel::new())),
        "discord" => Some(Box::new(DiscordChannel::new())),
        "zoom" => Some(Box::new(ZoomChannel::new())),
        "webex" => Some(Box::new(WebexChannel::new())),
        "whatsapp" => Some(Box::new(WhatsAppChannel::new())),
        "3cx" => Some(Box::new(ThreeCXChannel::new())),
        "ringcentral" => Some(Box::new(RingCentralChannel::new())),
        "dialpad" => Some(Box::new(DialpadChannel::new())),
        "twilio" => Some(Box::new(TwilioChannel::new())),
        "google_voice" | "google-voice" | "googlevoice" => {
            Some(Box::new(GoogleVoiceChannel::new()))
        }
        "webhook" => Some(Box::new(WebhookChannel::new())),
        _ => None,
    }
}
