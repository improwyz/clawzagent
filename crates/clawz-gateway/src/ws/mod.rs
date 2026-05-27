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
//! | `/rooms/{room_id}`              | Multi-participant agent room event stream             |
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

    // ── Multi-participant room events ───────────────────────────────────────

    /// A new message was appended to the room transcript.
    MessageAppend {
        room_id: String,
        seq: u64,
        message: serde_json::Value,
    },
    /// An existing room message was partially updated (streaming patch).
    MessagePatch {
        room_id: String,
        seq: u64,
        message_id: String,
        patch: serde_json::Value,
    },
    /// A participant started or stopped typing.
    Typing {
        room_id: String,
        participant_id: String,
        is_typing: bool,
    },
    /// A participant's presence status changed.
    Presence {
        room_id: String,
        participant_id: String,
        status: String,
    },
    /// Orchestration state for the room changed (agent routing, turn order, etc.).
    OrchestrationUpdate {
        room_id: String,
        update: serde_json::Value,
    },
    /// A governance approval is required before the room can proceed.
    ApprovalRequired {
        room_id: String,
        approval_id: String,
        details: serde_json::Value,
    },
    /// Client/server sequence numbers diverged; client should resync.
    SeqGap {
        room_id: String,
        expected_seq: u64,
        received_seq: u64,
    },
}

/// Room-scoped events emitted on `/ws/rooms/{room_id}`.
///
/// Alias for the room-related variants of [`WsEvent`]; used by room handlers
/// and REST hooks for clearer typing at call sites.
pub type RoomEvent = WsEvent;

/// Inbound control message sent by room WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RoomInbound {
    /// Broadcast typing indicator to other room participants.
    Typing {
        participant_id: String,
        is_typing: bool,
    },
    /// Broadcast presence change to other room participants.
    Presence {
        participant_id: String,
        status: String,
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
pub fn ws_routes() -> Router<crate::AppState> {
    Router::new()
        // Dependency: handlers::agent_stream — LLM streaming handler.
        .route("/agent_stream", get(handlers::agent_stream))
        // Web UI expects `/ws/agent_stream/{id}` for per-agent streaming.
        .route("/agent_stream/{id}", get(handlers::autonomous_stream))
        // Legacy alias kept for older clients.
        .route("/agent/{id}/stream", get(handlers::agent_stream))
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
        // Dependency: handlers::room_stream — multi-participant room handler.
        .route("/rooms/{room_id}", get(handlers::room_stream))
}
