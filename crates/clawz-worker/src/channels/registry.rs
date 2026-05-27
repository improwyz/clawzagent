//! Channel Registry — central lookup table for active channel plugins.
//!
//! The `ChannelRegistry` maintains two indexes:
//!
//! 1. `by_id`      — direct UUID → plugin + status mapping.
//! 2. `by_platform` — platform name (e.g. "slack") → list of channel UUIDs,
//!    because one platform may have multiple configured channels (two Slack
//!    workspaces, three Discord guilds, etc.).
//!
//! When the runtime wants to send a message it asks the registry for the
//! first *Ready* channel matching the requested platform.  If a channel
//! encounters an unrecoverable error the registry marks it `Error` so the
//! runtime can skip it on subsequent attempts.
//!
//! This module depends on `clawz_core::traits::ChannelPlugin` and
//! `clawz_core::types::channel::ChannelConfig`.

// ── Channel Registry ──────────────────────────────────────────────────────────

use std::collections::HashMap;

// Dependency: clawz_core::error — shared error types used across all crates.
use clawz_core::error::{ClawzError, Result};
// Dependency: clawz_core::traits::ChannelPlugin — trait that every plugin must implement.
use clawz_core::traits::ChannelPlugin;
// Dependency: clawz_core::types::channel::ChannelConfig — serialised channel configuration.
use clawz_core::types::channel::ChannelConfig;
use uuid::Uuid;

/// Operational health of a registered channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelStatus {
    /// Channel is healthy and may accept traffic.
    Ready,
    /// Channel was explicitly disabled in configuration.
    Disabled,
    /// Channel encountered a fatal error; `msg` records the reason so
    /// operators can diagnose without restarting the worker.
    Error(String),
}

/// Internal record stored for every registered channel.
struct Entry {
    /// The live plugin instance.  Boxed because ChannelPlugin is a trait object
    /// and the registry owns a heterogeneous collection of channel types.
    plugin: Box<dyn ChannelPlugin>,
    /// Current health state used by the runtime to decide whether this
    /// channel can accept a message.
    status: ChannelStatus,
    /// Platform identifier from the channel config (e.g. "slack", "discord").
    /// Duplicated here so `unregister` can clean up `by_platform` without
    /// needing to keep the original `ChannelConfig`.
    platform: String,
}

/// Central registry that maps a `channel_id` (UUID) to a `ChannelPlugin`.
///
/// Channels are registered with a `ChannelConfig` and can be looked up by
/// their UUID or by the platform name (e.g. "slack").
pub struct ChannelRegistry {
    /// UUID → entry.  Primary index; every registered channel lives here.
    by_id: HashMap<Uuid, Entry>,
    /// Platform name → list of channel ids (one platform may have multiple
    /// configured channels, e.g. two Slack workspaces).
    by_platform: HashMap<String, Vec<Uuid>>,
}

impl ChannelRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            by_id: HashMap::new(),
            by_platform: HashMap::new(),
        }
    }

    /// Register a plugin.  The channel's UUID comes from `config.id`.
    ///
    /// If a channel with the same UUID already exists it is silently
    /// overwritten; callers should `unregister` first if they need
    /// deterministic cleanup.
    pub fn register(&mut self, plugin: Box<dyn ChannelPlugin>, config: &ChannelConfig) {
        let id = config.id;
        let platform = config.platform.clone();

        // Maintain the secondary index so platform-scoped lookups stay O(1)
        // amortised even when many channels share the same platform.
        self.by_platform
            .entry(platform.clone())
            .or_default()
            .push(id);

        self.by_id.insert(
            id,
            Entry {
                plugin,
                // Derive the initial status from the config flag so disabled
                // channels are never selected by `get_by_platform`.
                status: if config.enabled {
                    ChannelStatus::Ready
                } else {
                    ChannelStatus::Disabled
                },
                platform,
            },
        );
    }

    /// Unregister a channel by UUID.  Returns `true` if it was present.
    ///
    /// Both `by_id` and `by_platform` are updated so the channel can no
    /// longer be discovered by any lookup path.
    pub fn unregister(&mut self, id: Uuid) -> bool {
        if let Some(entry) = self.by_id.remove(&id) {
            // Remove the UUID from the platform's list.  We do not shrink the
            // Vec eagerly; `get_by_platform` skips gaps with `find`, and
            // repeated churn would cause unnecessary reallocations.
            if let Some(ids) = self.by_platform.get_mut(&entry.platform) {
                ids.retain(|x| *x != id);
            }
            true
        } else {
            false
        }
    }

    /// Look up a plugin by its channel UUID.
    pub fn get(&self, id: Uuid) -> Option<&dyn ChannelPlugin> {
        self.by_id.get(&id).map(|e| e.plugin.as_ref())
    }

    /// Look up a plugin by its channel UUID (mutable).
    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut Box<dyn ChannelPlugin>> {
        self.by_id.get_mut(&id).map(|e| &mut e.plugin)
    }

    /// Return the first ready channel for `platform` (e.g. "slack").
    ///
    /// The search is left-to-right over the platform's UUID list, so
    /// registration order defines fallback priority.
    pub fn get_by_platform(&self, platform: &str) -> Option<&dyn ChannelPlugin> {
        self.by_platform
            .get(platform)?
            .iter()
            .find(|id| {
                self.by_id
                    .get(id)
                    .is_some_and(|e| e.status == ChannelStatus::Ready)
            })
            .and_then(|id| self.by_id.get(id))
            .map(|e| e.plugin.as_ref())
    }

    /// List all registered channels as `(uuid, platform, status)`.
    pub fn list(&self) -> Vec<(Uuid, &str, &ChannelStatus)> {
        self.by_id
            .iter()
            .map(|(id, e)| (*id, e.platform.as_str(), &e.status))
            .collect()
    }

    /// Mark a channel as errored.
    ///
    /// Once errored the channel will no longer be returned by
    /// `get_by_platform` until `set_ready` is called.
    pub fn set_error(&mut self, id: Uuid, msg: impl Into<String>) -> Result<()> {
        self.by_id
            .get_mut(&id)
            .ok_or_else(|| ClawzError::NotFound {
                entity: "channel".into(),
                id: id.to_string(),
            })
            .map(|e| e.status = ChannelStatus::Error(msg.into()))
    }

    /// Re-enable a channel that was previously disabled or errored.
    pub fn set_ready(&mut self, id: Uuid) -> Result<()> {
        self.by_id
            .get_mut(&id)
            .ok_or_else(|| ClawzError::NotFound {
                entity: "channel".into(),
                id: id.to_string(),
            })
            .map(|e| e.status = ChannelStatus::Ready)
    }

    /// Number of channels currently registered.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Returns `true` if no channels are registered.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}
