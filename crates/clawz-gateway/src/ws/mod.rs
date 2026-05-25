//! WebSocket route mount point for the ClawZ Gateway.
//!
//! This module exposes all real-time communication endpoints used by agents,
//! dashboards, and operators. Each route upgrades an HTTP connection to a
//! persistent WebSocket and delegates to a handler in the `handlers` submodule.
//!
//! # Routes
//!
//! | Path            | Purpose                                               |
//! |-----------------|-------------------------------------------------------|
//! | `/agent_stream` | Bidirectional LLM-style streaming (prompt → tokens)   |
//! | `/events`       | Subscribed agent lifecycle & system events            |
//! | `/metrics`      | Live operational metrics (CPU, memory, latency, etc.) |
//! | `/approvals`    | Human-in-the-loop approval requests & decisions       |
//! | `/logs`         | Structured log tailing from workers & gateway         |
//! | `/voice`        | Duplex binary audio frame exchange                    |
//!
//! # Dependencies
//!
//! - `axum` for HTTP routing and WebSocket upgrade support.
//!
//! # Related modules
//!
//! - `crate::ws::handlers` — contains the per-route WebSocket logic.

// Dependency: handlers submodule implements the actual WebSocket state machines.
pub mod handlers;

// Dependency: axum Router and GET method used for route registration.
use axum::{routing::get, Router};

/// Returns a `Router` with all WebSocket upgrade routes registered.
///
/// Mount this router under a common prefix (e.g. `/ws`) in the main Axum app.
pub fn ws_routes() -> Router {
    Router::new()
        // Dependency: handlers::agent_stream — LLM streaming handler.
        .route("/agent_stream", get(handlers::agent_stream))
        // Dependency: handlers::events — real-time event broadcast handler.
        .route("/events", get(handlers::events))
        // Dependency: handlers::metrics — operational metrics stream handler.
        .route("/metrics", get(handlers::metrics))
        // Dependency: handlers::approvals — approval workflow handler.
        .route("/approvals", get(handlers::approvals))
        // Dependency: handlers::logs — structured log tail handler.
        .route("/logs", get(handlers::logs))
        // Dependency: handlers::voice — duplex audio frame handler.
        .route("/voice", get(handlers::voice))
}
