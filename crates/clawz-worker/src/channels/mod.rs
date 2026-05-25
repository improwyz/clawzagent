//! Channel plugin system for the Clawz worker execution layer.
//!
//! This module implements the 3-tier architecture's channel abstraction,
//! sitting between the gateway (API layer) and the core (shared types/traits).
//! It provides:
//!
//! - `plugin`  – SDK helpers for writing channel plugins (rate limiting, retries,
//!   markdown conversion, credential extraction).
//! - `loader`  – Dynamic shared-library loader with hot-reload support.
//! - `registry` – In-memory index mapping channel UUIDs and platform names to
//!   live plugin instances and their health status.
//! - `native`  – Built-in channel implementations shipped with the worker.
//!
//! Workers execute via the runtime module; channels are the outbound pathway
//! through which the worker communicates with external platforms (Slack,
//! Discord, e-mail, etc.).
//!
//! # Dependency graph
//!
//! ```text
//! gateway → worker::channels → core::traits::ChannelPlugin
//! ```

// Dependency: clawz_core::traits::ChannelPlugin — shared trait all plugins implement.
pub mod plugin;
/// Dynamic plugin loader — loads `.so`/`.dylib` channel plugins at runtime.
pub mod loader;
/// In-memory registry — maps channel IDs and platform names to active plugin instances.
pub mod registry;
/// Built-in native channel implementations (e.g. Slack, Discord) compiled into the worker binary.
pub mod native;
