//! SSE stream of turn events for a single agent run.

use std::convert::Infallible;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode},
    response::Response,
};
use serde_json::Value;
use tokio_stream::StreamExt as TokioStreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::{AppState, GatewayError};

/// `GET /api/v1/agents/{id}/runs/{run_id}/events` — Server-Sent Events for one run.
pub async fn run_events_sse(
    State(state): State<AppState>,
    Path((agent_id, run_id)): Path<(String, String)>,
) -> Result<Response, GatewayError> {
    let _agent_id = agent_id;
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let rx = platform.events.subscribe();
    let run_id_filter = run_id.clone();

    let body_stream = TokioStreamExt::filter_map(BroadcastStream::new(rx), move |msg| {
        let run_id_filter = run_id_filter.clone();
        let raw = msg.ok()?;
        let envelope: Value = serde_json::from_str(&raw).ok()?;
        let data = envelope
            .get("data")
            .cloned()
            .unwrap_or_else(|| envelope.clone());
        let event_run_id = data
            .get("run_id")
            .and_then(|v| v.as_str())
            .or_else(|| envelope.get("run_id").and_then(|v| v.as_str()));
        if event_run_id != Some(run_id_filter.as_str()) {
            return None;
        }
        let event_type = envelope
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("agent.turn.event");
        let frame = format!("event: {event_type}\ndata: {raw}\n\n");
        Some(Ok::<String, Infallible>(frame))
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        )
        .header(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        )
        .body(Body::from_stream(body_stream))
        .map_err(|e| GatewayError::Internal(e.to_string()))
}
