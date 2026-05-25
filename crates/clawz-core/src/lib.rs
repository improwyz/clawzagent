//! clawz-core — shared primitives for the ClawZ agent platform.
//!
//! This crate sits at the bottom of the 3-crate architecture:
//!   * `clawz-core`   — types, traits, config, errors, metrics (this crate)
//!   * `clawz-worker` — runtime engine: agent scheduler, provider routing,
//!                      tool orchestration, pipeline execution, mesh networking
//!   * `clawz-gateway` — external API surface: REST/gRPC, webhooks, channel
//!                       adapters, auth, rate-limiting
//!
//! Any code that needs to be visible to **both** worker and gateway lives here.
//! That includes:
//!   - `AppConfig` and TOML/env-loading logic
//!   - All async trait interfaces (Provider, PipelineStep, ChannelPlugin, …)
//!   - Database models and repository helpers (sqlx/Postgres)
//!   - The canonical `ClawzError` enum and `Result` alias
//!   - Prometheus-compatible metrics primitives
//!   - Lock-free circuit breaker for provider resilience
//!
//! Dependency direction is strictly upward: core → (worker, gateway).
//! Core must never depend on worker or gateway crates.

pub mod circuit_breaker;
pub mod config;
pub mod db;
pub mod deployment;
pub mod error;
pub mod metrics;
pub mod traits;
pub mod types;

// Re-export the two items every downstream crate needs immediately.
pub use config::AppConfig;
pub use error::{ClawzError, Result};
