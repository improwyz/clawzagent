//! Platform persistence trait — implemented by gateway Postgres store.

use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Database(String),
    #[error("not found: {resource} {id}")]
    NotFound { resource: String, id: String },
}

pub type StoreResult<T> = Result<T, StoreError>;

/// CRUD facade over clawz-core repositories (gateway implementation).
#[async_trait]
pub trait PlatformStore: Send + Sync {
    async fn list_agents_json(&self) -> StoreResult<Value>;
    async fn get_agent_json(&self, id: &str) -> StoreResult<Value>;
}
