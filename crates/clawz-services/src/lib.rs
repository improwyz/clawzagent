//! Shared platform services for ClawZ — execution client, events, and DTOs.

pub mod dto;
pub mod events;
pub mod execution;
pub mod platform;
pub mod store;

pub use dto::*;
pub use events::EventBus;
pub use execution::{ExecutionClient, ExecutionError, HttpExecutionClient};
pub use platform::Platform;
pub use store::{PlatformStore, StoreError, StoreResult};
