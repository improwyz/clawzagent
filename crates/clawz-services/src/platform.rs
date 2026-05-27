//! Top-level platform facade used by gateway handlers.

use std::sync::Arc;

use crate::events::EventBus;
use crate::execution::ExecutionClient;

/// Shared services injected into gateway `AppState`.
pub struct Platform {
    pub execution: Arc<dyn ExecutionClient>,
    pub events: EventBus,
}

impl Platform {
    pub fn new(execution: Arc<dyn ExecutionClient>) -> Self {
        Self {
            execution,
            events: EventBus::new(),
        }
    }
}
