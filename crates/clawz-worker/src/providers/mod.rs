//! Provider subsystem for the ClawZ worker.
//!
//! This module manages multiple LLM providers (OpenAI, Anthropic, Gemini,
//! Azure, Bedrock, DeepSeek, Ollama) via a unified routing layer that
//! supports circuit breakers, retries, cost tracking, and fallback chains.
//!
//! ## Sub-modules
//!
//! - [`adapters`] — Provider-specific HTTP adapters implementing [`ProviderAdapter`].
//! - [`config`]   — TOML / environment-variable configuration loaders.
//! - [`cost`]     — Per-call cost tracking and budget enforcement.
//! - [`registry`] — Model-name → provider resolution and fallback chains.
//! - [`router`]   — Intelligent request routing with circuit breaker and retries.
//!
//! // Dependency: `clawz_core::traits::Provider` defines the contract that
//! // each adapter ultimately satisfies for the worker.

pub mod adapters;
pub mod config;
pub mod cost;
pub mod registry;
pub mod router;

// Re-export the public API surface so callers can `use providers::*`.
pub use adapters::ProviderAdapter;
pub use config::{config_from_env, load_provider_config, save_provider_config};
pub use cost::CostTracker;
pub use registry::ProviderRegistry;
pub use router::{
    AuthType, ProviderConfig, ProviderRouter, ProviderRouterConfig, ReliabilityConfig,
};
