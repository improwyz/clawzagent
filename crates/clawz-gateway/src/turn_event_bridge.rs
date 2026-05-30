//! Forward worker [`TurnEvent`](clawz_worker::runtime::turn_events::TurnEvent)s to the platform event bus.

use std::sync::Arc;

use clawz_services::Platform;
use clawz_worker::runtime::turn_events::TurnEvent;
use clawz_worker::service::WorkerService;

/// Subscribe to the in-process worker turn bus and publish JSON events for WebSocket clients.
pub fn spawn_turn_event_bridge(service: Arc<WorkerService>, platform: Arc<Platform>) {
    let mut rx = service.turn_event_bus().subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let (event_type, payload) = turn_event_to_envelope(&ev);
                    platform.events.publish_json(event_type, payload);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn turn_event_to_envelope(ev: &TurnEvent) -> (&'static str, serde_json::Value) {
    match ev {
        TurnEvent::ProviderDelta {
            run_id,
            conversation_id,
            delta,
        } => (
            "agent.turn.provider_delta",
            serde_json::json!({ "run_id": run_id, "conversation_id": conversation_id, "delta": delta }),
        ),
        TurnEvent::ToolStart {
            run_id,
            conversation_id,
            tool_name,
            tool_call_id,
        } => (
            "agent.turn.tool_start",
            serde_json::json!({
                "run_id": run_id,
                "conversation_id": conversation_id,
                "tool_name": tool_name,
                "tool_call_id": tool_call_id,
            }),
        ),
        TurnEvent::ToolEnd {
            run_id,
            conversation_id,
            tool_name,
            tool_call_id,
            success,
            output_preview,
        } => (
            "agent.turn.tool_end",
            serde_json::json!({
                "run_id": run_id,
                "conversation_id": conversation_id,
                "tool_name": tool_name,
                "tool_call_id": tool_call_id,
                "success": success,
                "output_preview": output_preview,
            }),
        ),
        TurnEvent::GovernanceHold {
            run_id,
            conversation_id,
            reason,
        } => (
            "agent.turn.governance_hold",
            serde_json::json!({ "run_id": run_id, "conversation_id": conversation_id, "reason": reason }),
        ),
        TurnEvent::TurnComplete {
            run_id,
            conversation_id,
            turn_count,
        } => (
            "agent.turn.complete",
            serde_json::json!({
                "run_id": run_id,
                "conversation_id": conversation_id,
                "turn_count": turn_count,
            }),
        ),
        TurnEvent::Error {
            run_id,
            conversation_id,
            message,
        } => (
            "agent.turn.error",
            serde_json::json!({ "run_id": run_id, "conversation_id": conversation_id, "message": message }),
        ),
    }
}
