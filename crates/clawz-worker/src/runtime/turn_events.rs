//! Turn-scoped event stream for agent runs (tool start/end, provider deltas).
//!
//! Emitted during pipeline execution so gateways and dashboards can subscribe
//! instead of synthesizing placeholder WebSocket events.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Lifecycle events for a single agent run / conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    ProviderDelta {
        run_id: String,
        conversation_id: String,
        delta: String,
    },
    ToolStart {
        run_id: String,
        conversation_id: String,
        tool_name: String,
        tool_call_id: String,
    },
    ToolEnd {
        run_id: String,
        conversation_id: String,
        tool_name: String,
        tool_call_id: String,
        success: bool,
        output_preview: String,
    },
    GovernanceHold {
        run_id: String,
        conversation_id: String,
        reason: String,
    },
    TurnComplete {
        run_id: String,
        conversation_id: String,
        turn_count: u32,
    },
    Error {
        run_id: String,
        conversation_id: String,
        message: String,
    },
}

/// Broadcast bus for turn events keyed by run id.
#[derive(Clone)]
pub struct TurnEventBus {
    sender: broadcast::Sender<TurnEvent>,
}

impl TurnEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(16));
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TurnEvent> {
        self.sender.subscribe()
    }

    pub fn emit(&self, event: TurnEvent) {
        let _ = self.sender.send(event);
    }

    pub fn new_run_id() -> String {
        Uuid::new_v4().to_string()
    }
}

impl Default for TurnEventBus {
    fn default() -> Self {
        Self::new(256)
    }
}
