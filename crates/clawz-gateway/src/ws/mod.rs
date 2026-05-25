//! WebSocket route mount point for the ClawZ Gateway.
//!
//! This module exposes all real-time communication endpoints used by agents,
//! dashboards, and operators. Each route upgrades an HTTP connection to a
//! persistent WebSocket and delegates to a handler in the `handlers` submodule.
//!
//! # Routes
//!
//! | Path                            | Purpose                                               |
//! |---------------------------------|-------------------------------------------------------|
//! | `/agent_stream`                 | Bidirectional LLM-style streaming (prompt → tokens)   |
//! | `/agents/{id}/stream`           | Autonomous multi-turn activity stream                 |
//! | `/events`                       | Subscribed agent lifecycle & system events            |
//! | `/metrics`                      | Live operational metrics (CPU, memory, latency, etc.) |
//! | `/approvals`                    | Human-in-the-loop approval requests & decisions       |
//! | `/logs`                         | Structured log tailing from workers & gateway         |
//! | `/voice`                        | Duplex binary audio frame exchange                    |
//!
//! # Dependencies
//!
//! - `axum` for HTTP routing and WebSocket upgrade support.
//! - `serde` for the [`WsEvent`] wire format.
//!
//! # Related modules
//!
//! - `crate::ws::handlers` — contains the per-route WebSocket logic.

// Dependency: handlers submodule implements the actual WebSocket state machines.
pub mod handlers;

// Dependency: axum Router and GET method used for route registration.
use axum::{routing::get, Router};
use serde::{Deserialize, Serialize};

/// Wire-format event emitted by the autonomous-activity WebSocket stream.
///
/// Each variant is tagged via `serde(tag = "type")` so clients receive
/// self-describing JSON like:
///
/// ```json
/// { "type": "TurnStart", "turn": 0 }
/// { "type": "TurnComplete", "turn": 0, "output": "..." }
/// { "type": "CostBudgetExceeded", "accumulated_usd": 1.23 }
/// { "type": "SessionEnd", "total_turns": 3, "total_cost_usd": 0.42 }
/// { "type": "Error", "error": "..." }
/// ```
///
/// The actual emission of these events from `run_multi_turn` is deferred to a
/// follow-up integration; this enum nails down the wire shape so clients and
/// dashboards can be built against it now.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsEvent {
    /// A new autonomous turn has started.
    TurnStart {
        /// Zero-indexed turn counter within the session.
        turn: usize,
    },
    /// The current autonomous turn has completed successfully.
    TurnComplete {
        /// Zero-indexed turn counter within the session.
        turn: usize,
        /// Textual output produced by the turn (assistant message, tool result, etc.).
        output: String,
    },
    /// The agent's accumulated cost has crossed the configured budget ceiling.
    ///
    /// Receipt of this event implies the session will terminate shortly after.
    CostBudgetExceeded {
        /// Total USD spent across all turns in this session.
        accumulated_usd: f64,
    },
    /// The autonomous session has ended (budget exhausted, max turns, or stop).
    SessionEnd {
        /// Total number of turns executed.
        total_turns: usize,
        /// Total USD spent across the session.
        total_cost_usd: f64,
    },
    /// A non-fatal error occurred during the session.
    Error {
        /// Human-readable error description.
        error: String,
    },
}

/// Inbound control message received by the autonomous-activity stream.
///
/// The client sends `{"type":"start"}` to begin emitting events or
/// `{"type":"stop"}` to end the session early.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WsControl {
    /// Begin emitting autonomous activity events.
    Start,
    /// Terminate the autonomous session.
    Stop,
}

/// Returns a `Router` with all WebSocket upgrade routes registered.
///
/// Mount this router under a common prefix (e.g. `/ws`) in the main Axum app.
pub fn ws_routes() -> Router {
    Router::new()
        // Dependency: handlers::agent_stream — LLM streaming handler.
        .route("/agent_stream", get(handlers::agent_stream))
        // Dependency: handlers::autonomous_stream — autonomous multi-turn handler.
        .route(
            "/agents/{id}/stream",
            get(handlers::autonomous_stream),
        )
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
