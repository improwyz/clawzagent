//! WebSocket handler implementations for the ClawZ Gateway.
//!
//! Each handler follows the same pattern:
//! 1. An outer `pub async fn` accepts `WebSocketUpgrade` and calls `on_upgrade`.
//! 2. An inner `async fn` runs the actual WebSocket loop until the peer disconnects.
//!
//! All handlers are currently **demo / stub implementations** that stream canned data
//! on deterministic timers so dashboards and clients have something to render while
//! the back-end integrations are being built.
//!
//! # Cross-crate dependencies
//!
//! - `axum` — WebSocket upgrade, message framing, ping/pong.
//! - `chrono` — RFC-3339 timestamps on every outbound message.
//! - `serde_json` — ad-hoc JSON payload construction.
//! - `tokio` — async runtime, `select!` for concurrent timers and socket I/O.

use crate::AppState;
use crate::auth::resolve_request_auth;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
// Dependency: chrono provides UTC timestamps for every outbound message.
use chrono::Utc;
// Dependency: serde_json used for lightweight JSON payload construction.
use serde_json::json;
// Dependency: tokio time utilities for interval-driven demo data.
use tokio::time::{Duration, interval};
// Dependency: WsEvent / WsControl / RoomInbound wire shapes for streaming.
use super::{RoomInbound, WsControl, WsEvent};

// ---------------------------------------------------------------------------
// Helper macros
// ---------------------------------------------------------------------------

/// Build a `Message::Text` from a JSON value.
///
/// axum 0.8 requires `Utf8Bytes` instead of a plain `String`, so we convert
/// via `.into()`. Using a macro keeps call-sites tidy.
macro_rules! text_msg {
    ($val:expr) => {
        Message::Text($val.to_string().into())
    };
}

// ---------------------------------------------------------------------------
// agent_stream — bidirectional LLM-style token streaming
// ---------------------------------------------------------------------------

/// Upgrade an HTTP connection to a WebSocket that accepts a prompt and streams
/// back tokens word-by-word, concluding with a `done` payload.
pub async fn agent_stream(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_agent_stream)
}

/// Echoes the prompt back as a simulated LLM response.
///
/// Protocol:
/// - Inbound: JSON `{"prompt": "..."}` or raw text.
/// - Outbound:
///   - `{"type":"status","data":"thinking"}`
///   - `{"type":"token","data":"<word>"}`  (one per word, 40 ms apart)
///   - `{"type":"done","data":{...}}`
async fn handle_agent_stream(mut socket: WebSocket) {
    while let Some(Ok(msg)) = socket.recv().await {
        let user_text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };

        let prompt = serde_json::from_str::<serde_json::Value>(&user_text)
            .ok()
            .and_then(|v| v["prompt"].as_str().map(|s| s.to_string()))
            // Why: if the client sends malformed JSON or omits the prompt field,
            // we still want to echo the raw payload rather than silently failing.
            .unwrap_or(user_text);

        // Notify the client that "work" has started; this keeps the UI spinner alive.
        if socket
            .send(text_msg!(json!({"type": "status", "data": "thinking"})))
            .await
            .is_err()
        {
            // Peer disappeared before we could send; end the loop cleanly.
            break;
        }

        // Build a canned response and stream it word by word.
        let response = format!(
            "I received your message: \"{}\". Here is a streamed response from ClawZ.",
            prompt
        );
        let words: Vec<&str> = response.split_whitespace().collect();
        let total = words.len();

        for word in &words {
            let token_msg = json!({"type": "token", "data": word});
            if socket.send(text_msg!(token_msg)).await.is_err() {
                // Connection dropped mid-stream; no need to send remaining tokens.
                return;
            }
            // Why: 40 ms yields ~25 tokens/sec, a plausible "fast LLM" feel for demos.
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        let done = json!({
            "type": "done",
            "data": {
                "total_tokens": total,
                "model": "clawz-internal",
                "finish_reason": "stop"
            }
        });
        if socket.send(text_msg!(done)).await.is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// events — periodic agent lifecycle & system events
// ---------------------------------------------------------------------------

/// Upgrade an HTTP connection to a WebSocket that pushes canned agent events
/// every 5 seconds.
pub async fn events(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_events)
}

/// Streams a repeating cycle of agent status changes.
///
/// Protocol:
/// - Outbound: `{"type":"connected",...}` on open, then periodic
///   `{"type":"agent_status|task_complete|error", "agent_id":"...", "status":"..."}`.
async fn handle_events(mut socket: WebSocket) {
    let welcome = json!({
        "type": "connected",
        "data": {"message": "Subscribed to ClawZ event stream"},
        "timestamp": Utc::now().to_rfc3339()
    });
    if socket.send(text_msg!(welcome)).await.is_err() {
        return;
    }

    // Why: a fixed array is the simplest way to cycle through demo events deterministically.
    let agent_events = [
        ("agent_status", "agent-001", "running"),
        ("agent_status", "agent-002", "idle"),
        ("task_complete", "agent-001", "done"),
        ("agent_status", "agent-003", "starting"),
        ("error", "agent-002", "timeout"),
    ];
    let mut tick = interval(Duration::from_secs(5));
    let mut idx = 0usize;

    loop {
        // Why: `select!` lets us emit ticks AND react to peer control messages
        // (Close, Ping) on the same task without spawning extra threads.
        tokio::select! {
            _ = tick.tick() => {
                let (ev_type, agent_id, status) = agent_events[idx % agent_events.len()];
                idx += 1;
                let event = json!({
                    "type": ev_type,
                    "agent_id": agent_id,
                    "status": status,
                    "timestamp": Utc::now().to_rfc3339()
                });
                if socket.send(text_msg!(event)).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Ping(data))) => {
                        // RFC 6455 requires us to reply with a matching Pong.
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// metrics — jittered operational metrics
// ---------------------------------------------------------------------------

/// Upgrade an HTTP connection to a WebSocket that pushes simulated gateway
/// metrics every 2 seconds.
pub async fn metrics(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_metrics)
}

/// Streams synthetic operational metrics that jitter within realistic bounds.
///
/// Protocol:
/// - Outbound: `{"type":"metrics","timestamp":"...","data":{...}}`
async fn handle_metrics(mut socket: WebSocket) {
    let mut tick = interval(Duration::from_secs(2));
    // Why: start with "plausible" demo values so the first frame isn't all zeros.
    let mut active_agents: u32 = 3;
    let mut rpm: u32 = 42;
    let mut latency: u32 = 120;
    let mut memory: u32 = 512;
    let mut cpu: u32 = 25;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Why: deterministic pseudo-random walk keeps the demo dashboard
                // visually alive without needing an external metrics source.
                active_agents = (active_agents + 1) % 12;
                rpm = (rpm + 7) % 200;
                latency = 50 + (latency + 13) % 150;
                memory = 256 + (memory + 17) % 512;
                cpu = (cpu + 5) % 80;

                let payload = json!({
                    "type": "metrics",
                    "timestamp": Utc::now().to_rfc3339(),
                    "data": {
                        "active_agents": active_agents,
                        "requests_per_min": rpm,
                        "avg_latency_ms": latency,
                        "memory_mb": memory,
                        "cpu_percent": cpu,
                    }
                });
                if socket.send(text_msg!(payload)).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// approvals — human-in-the-loop approval workflow
// ---------------------------------------------------------------------------

/// Upgrade an HTTP connection to a WebSocket that shows pending approvals and
/// acknowledges client decisions.
pub async fn approvals(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_approvals)
}

/// Pushes a snapshot of pending approvals, then re-broadcasts them periodically.
///
/// Also accepts inbound JSON decision messages and echoes them back as `approval_ack`.
///
/// Protocol:
/// - Outbound: `{"type":"approvals_snapshot","data":[...]}` followed by
///   `{"type":"approval_pending","data":{...}}` every 3 s.
/// - Inbound:  `{"id":"...","decision":"approve|reject"}` (optional).
async fn handle_approvals(mut socket: WebSocket) {
    // Why: hard-coded demo queue so the UI has something to render immediately.
    let pending = vec![
        json!({"id": "appr-001", "agent_id": "agent-001", "action": "deploy", "resource": "prod-cluster", "risk": "high"}),
        json!({"id": "appr-002", "agent_id": "agent-002", "action": "delete", "resource": "old-snapshots", "risk": "medium"}),
    ];
    let mut tick = interval(Duration::from_secs(3));
    let mut idx = 0usize;

    // Send the full queue once so the client can render the complete list.
    let snapshot = json!({
        "type": "approvals_snapshot",
        "data": pending,
        "timestamp": Utc::now().to_rfc3339()
    });
    if socket.send(text_msg!(snapshot)).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Cycle through the same items so the demo never runs dry.
                let item = &pending[idx % pending.len()];
                idx += 1;
                let update = json!({
                    "type": "approval_pending",
                    "data": item,
                    "timestamp": Utc::now().to_rfc3339()
                });
                if socket.send(text_msg!(update)).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Why: the client may send lightweight decision JSON;
                        // parsing lets us echo a structured ack. Failure is
                        // harmless—malformed messages are simply dropped.
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.as_str()) {
                            let ack = json!({
                                "type": "approval_ack",
                                "id": v["id"],
                                "decision": v["decision"],
                                "timestamp": Utc::now().to_rfc3339()
                            });
                            let _ = socket.send(text_msg!(ack)).await;
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// logs — structured log tailing
// ---------------------------------------------------------------------------

/// Upgrade an HTTP connection to a WebSocket that emits canned structured log
/// lines every 800 ms.
pub async fn logs(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_logs)
}

/// Cycles through a fixed set of JSON log lines to simulate log tailing.
///
/// Protocol:
/// - Outbound: `{"type":"log_line","timestamp":"...","data":{...}}`
async fn handle_logs(mut socket: WebSocket) {
    // Why: pre-canned JSON strings let us test both valid and edge-case parsing.
    let log_lines = [
        r#"{"level":"INFO","target":"clawz_worker","message":"Agent started","agent_id":"agent-001"}"#,
        r#"{"level":"DEBUG","target":"clawz_gateway","message":"Received request","path":"/api/agents"}"#,
        r#"{"level":"INFO","target":"clawz_worker","message":"Task dispatched","task_id":"task-042"}"#,
        r#"{"level":"WARN","target":"clawz_worker","message":"Provider rate limit approaching","provider":"openai"}"#,
        r#"{"level":"INFO","target":"clawz_gateway","message":"WebSocket client connected","channel":"logs"}"#,
        r#"{"level":"ERROR","target":"clawz_worker","message":"Tool execution failed","tool":"web_search","error":"timeout"}"#,
        r#"{"level":"INFO","target":"clawz_worker","message":"Agent task complete","agent_id":"agent-002","duration_ms":1240}"#,
    ];
    let mut tick = interval(Duration::from_millis(800));
    let mut idx = 0usize;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let line = log_lines[idx % log_lines.len()];
                idx += 1;
                let msg = json!({
                    "type": "log_line",
                    "timestamp": Utc::now().to_rfc3339(),
                    // Why: some log sources may emit plain text; wrapping it in a
                    // "raw" field keeps the schema consistent for the UI.
                    "data": serde_json::from_str::<serde_json::Value>(line)
                        .unwrap_or(json!({"raw": line}))
                });
                if socket.send(text_msg!(msg)).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// voice — duplex binary audio frame exchange
// ---------------------------------------------------------------------------

/// Upgrade an HTTP connection to a WebSocket that accepts binary audio frames
/// and echoes them back (duplex).
pub async fn voice(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_voice)
}

/// Receives binary audio frames and text control messages.
///
/// Protocol:
/// - Inbound binary → echoed back immediately (duplex loopback).
/// - Inbound text  → treated as control messages (`start`, `stop`, `config`).
/// - Outbound      → `{"type":"audio_frame_ack","bytes":N}` or
///   `{"type":"voice_ack","received":"..."}`.
async fn handle_voice(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Binary(data)) => {
                let byte_len = data.len();
                let meta = json!({
                    "type": "audio_frame_ack",
                    "bytes": byte_len,
                    "timestamp": Utc::now().to_rfc3339()
                });
                if socket.send(text_msg!(meta)).await.is_err() {
                    break;
                }
                // Why: echoing the binary back demonstrates real-time duplex
                // capability even before a real audio pipeline is integrated.
                if socket.send(Message::Binary(data)).await.is_err() {
                    break;
                }
            }
            Ok(Message::Text(text)) => {
                // Why: text messages carry control state (start/stop/config);
                // the actual audio payload stays in Binary frames.
                let received_str = text.as_str().to_string();
                let ack = json!({
                    "type": "voice_ack",
                    "received": received_str,
                    "timestamp": Utc::now().to_rfc3339()
                });
                if socket.send(text_msg!(ack)).await.is_err() {
                    break;
                }
            }
            Ok(Message::Ping(data)) => {
                let _ = socket.send(Message::Pong(data)).await;
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// autonomous_stream — multi-turn autonomous activity broadcast
// ---------------------------------------------------------------------------

/// Upgrade an HTTP connection to a WebSocket that broadcasts
/// [`WsEvent`](super::WsEvent)s emitted by an autonomous multi-turn agent run.
///
/// Mounted at `/ws/agents/{id}/stream`. The `{id}` segment is the agent's
/// unique identifier; it is captured via [`axum::extract::Path`] and made
/// available to the handler for future integration with the worker's
/// `run_multi_turn` pipeline.
///
/// # Protocol
///
/// **Inbound** (text JSON):
/// - `{"type":"start"}` — begin emitting events. Currently this is a no-op:
///   the handler begins emitting a canned [`WsEvent::TurnStart`] sequence as
///   soon as the upgrade completes. A follow-up commit will attach the
///   handler to a real `run_multi_turn` event channel keyed by `{id}`.
/// - `{"type":"stop"}` — terminate the session; the handler emits a final
///   [`WsEvent::SessionEnd`] and closes the socket with code 1000 (normal).
///
/// **Outbound** (text JSON, all variants of [`WsEvent`]).
///
/// # Future integration
///
/// The event channel that feeds this stream is intentionally deferred — see
/// Task 25 in the autonomous-activity plan. Once the worker exposes a
/// per-session event sender, the canned sequence below will be replaced with
/// `tokio::sync::broadcast::Receiver<WsEvent>` polling.
pub async fn autonomous_stream(Path(agent_id): Path<String>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_autonomous_stream(socket, agent_id))
}

/// Send a [`WsEvent`] as a text-frame JSON message.
///
/// Returns `false` if the peer has disconnected; callers should bail out of
/// their loop in that case.
async fn send_event(socket: &mut WebSocket, event: &WsEvent) -> bool {
    let json = match serde_json::to_string(event) {
        Ok(s) => s,
        Err(_) => return false,
    };
    socket.send(Message::Text(json.into())).await.is_ok()
}

/// Runs the autonomous-stream WebSocket loop for a single client.
///
/// The current implementation emits a fixed three-turn demo sequence with a
/// small delay between events so dashboards can wire up against a real
/// `WsEvent` stream today. The actual event source will be wired in a
/// follow-up commit once `run_multi_turn` exposes an event channel.
async fn handle_autonomous_stream(mut socket: WebSocket, agent_id: String) {
    // Greet the client so it knows the upgrade succeeded and which agent this
    // session is bound to. This is a plain JSON envelope, not a WsEvent, so
    // it never clashes with the typed event stream.
    let hello = json!({
        "type": "connected",
        "agent_id": agent_id,
        "timestamp": Utc::now().to_rfc3339(),
    });
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // Wait for the first control message — either {"type":"start"} to kick
    // things off, or {"type":"stop"} for an immediate clean shutdown.
    let mut started = false;
    while !started {
        let msg = match socket.recv().await {
            Some(Ok(m)) => m,
            _ => return,
        };
        match msg {
            Message::Text(text) => match serde_json::from_str::<WsControl>(text.as_str()) {
                Ok(WsControl::Start) => started = true,
                Ok(WsControl::Stop) => {
                    let _ = send_event(
                        &mut socket,
                        &WsEvent::SessionEnd {
                            total_turns: 0,
                            total_cost_usd: 0.0,
                        },
                    )
                    .await;
                    let _ = socket
                        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                            code: axum::extract::ws::close_code::NORMAL,
                            reason: "stop requested".into(),
                        })))
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = send_event(
                        &mut socket,
                        &WsEvent::Error {
                            error: format!("invalid control message: {e}"),
                        },
                    )
                    .await;
                }
            },
            Message::Ping(data) => {
                let _ = socket.send(Message::Pong(data)).await;
            }
            Message::Close(_) => return,
            _ => {}
        }
    }

    // Canned three-turn demo sequence. Replace with real event-channel polling
    // once `run_multi_turn` exposes an event sender keyed by agent_id.
    let canned_turns = [
        ("Planning the next step.", 0.012f64),
        ("Executing the planned action.", 0.018),
        ("Reviewing results and consolidating output.", 0.011),
    ];
    let mut total_cost = 0.0f64;

    for (turn_idx, (output, cost)) in canned_turns.iter().enumerate() {
        // Why: select! lets us emit ticks while still reacting to a client
        // {"type":"stop"} mid-session.
        let stopped = tokio::select! {
            stop = wait_for_stop(&mut socket) => stop,
            _ = tokio::time::sleep(Duration::from_millis(100)) => false,
        };
        if stopped {
            let _ = send_event(
                &mut socket,
                &WsEvent::SessionEnd {
                    total_turns: turn_idx,
                    total_cost_usd: total_cost,
                },
            )
            .await;
            let _ = socket
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: axum::extract::ws::close_code::NORMAL,
                    reason: "stop requested".into(),
                })))
                .await;
            return;
        }

        if !send_event(&mut socket, &WsEvent::TurnStart { turn: turn_idx }).await {
            return;
        }
        total_cost += cost;
        if !send_event(
            &mut socket,
            &WsEvent::TurnComplete {
                turn: turn_idx,
                output: output.to_string(),
            },
        )
        .await
        {
            return;
        }
    }

    let _ = send_event(
        &mut socket,
        &WsEvent::SessionEnd {
            total_turns: canned_turns.len(),
            total_cost_usd: total_cost,
        },
    )
    .await;
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: axum::extract::ws::close_code::NORMAL,
            reason: "session complete".into(),
        })))
        .await;
}

/// Drain pending inbound frames non-blockingly; return `true` if a `stop`
/// control message was observed. Pings are answered inline so heartbeats keep
/// working across long autonomous sessions.
async fn wait_for_stop(socket: &mut WebSocket) -> bool {
    match socket.recv().await {
        Some(Ok(Message::Text(text))) => matches!(
            serde_json::from_str::<WsControl>(text.as_str()),
            Ok(WsControl::Stop)
        ),
        Some(Ok(Message::Ping(data))) => {
            let _ = socket.send(Message::Pong(data)).await;
            false
        }
        Some(Ok(Message::Close(_))) | None => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// room_stream — multi-participant agent room events
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct RoomStreamQuery {
    pub api_key: Option<String>,
}

/// Upgrade an HTTP connection to a WebSocket subscribed to a room's event stream.
///
/// Mounted at `/ws/rooms/{room_id}`. Forwards [`WsEvent`] payloads published via
/// [`AppState::publish_room_event`] and relays inbound typing/presence control
/// messages to other subscribers in the same room.
pub async fn room_stream(
    State(state): State<AppState>,
    Path(room_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<RoomStreamQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let ctx = resolve_request_auth(&headers, query.api_key.as_deref())?;

    let member = {
        let rooms = state.rooms.read().await;
        rooms
            .iter()
            .find(|r| r.id == room_id)
            .map(|room| {
                room.participants
                    .iter()
                    .any(|p| p.participant_id == ctx.user_id)
            })
            .unwrap_or(false)
    };
    if !member {
        return Err(StatusCode::FORBIDDEN);
    }

    let rx = state.subscribe_room(&room_id).await;
    Ok(ws.on_upgrade(move |socket| handle_room_stream(socket, room_id, rx, state)))
}

/// Room WebSocket loop: forward broadcast events and handle inbound control.
async fn handle_room_stream(
    mut socket: WebSocket,
    room_id: String,
    mut rx: tokio::sync::broadcast::Receiver<String>,
    state: AppState,
) {
    let hello = json!({
        "type": "connected",
        "room_id": room_id,
        "timestamp": Utc::now().to_rfc3339(),
    });
    if socket.send(text_msg!(hello)).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(payload) => {
                        if socket.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let gap = serde_json::to_value(WsEvent::SeqGap {
                            room_id: room_id.clone(),
                            expected_seq: 0,
                            received_seq: 0,
                        })
                        .unwrap_or(json!({"type":"SeqGap","room_id":room_id}));
                        if socket.send(text_msg!(gap)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(inbound) = serde_json::from_str::<RoomInbound>(text.as_str()) {
                            let event = match inbound {
                                RoomInbound::Typing { participant_id, is_typing } => {
                                    WsEvent::Typing {
                                        room_id: room_id.clone(),
                                        participant_id,
                                        is_typing,
                                    }
                                }
                                RoomInbound::Presence { participant_id, status } => {
                                    WsEvent::Presence {
                                        room_id: room_id.clone(),
                                        participant_id,
                                        status,
                                    }
                                }
                            };
                            if let Ok(json) = serde_json::to_value(&event) {
                                state.publish_room_event(&room_id, json).await;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}
