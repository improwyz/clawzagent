//! Platform-wide event bus for WebSocket subscribers.

use serde_json::Value;
use tokio::sync::broadcast;

const EVENT_CAPACITY: usize = 512;

/// Best-effort broadcast bus; slow consumers are dropped.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<String>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(EVENT_CAPACITY);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    pub fn publish_json(&self, event_type: &str, payload: Value) {
        let envelope = serde_json::json!({
            "type": event_type,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "data": payload,
        });
        let _ = self.tx.send(envelope.to_string());
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
