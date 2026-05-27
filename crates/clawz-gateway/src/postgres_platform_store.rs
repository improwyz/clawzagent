//! Postgres-backed [`PlatformStore`] for gateway CRUD.

use std::sync::Arc;

use async_trait::async_trait;
use clawz_services::store::{PlatformStore, StoreError, StoreResult};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::AgentRecord;
use crate::postgres_store;

/// Persistent store delegating to `clawz-core` repositories.
pub struct PostgresPlatformStore {
    pool: PgPool,
}

impl PostgresPlatformStore {
    pub fn new(pool: PgPool) -> Arc<Self> {
        Arc::new(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Upsert an agent row and keep the in-memory registry in sync at call sites.
    pub async fn persist_agent_record(&self, record: &AgentRecord) -> StoreResult<()> {
        postgres_store::persist_agent(&self.pool, record)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))
    }

    /// Load all agents for startup hydration.
    pub async fn load_agent_records(&self) -> StoreResult<Vec<AgentRecord>> {
        postgres_store::load_agents(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))
    }
}

#[async_trait]
impl PlatformStore for PostgresPlatformStore {
    async fn list_agents_json(&self) -> StoreResult<Value> {
        let agents = self.load_agent_records().await?;
        let data: Vec<Value> = agents
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "name": a.name,
                    "model": a.model,
                    "status": a.status.to_string(),
                    "system_prompt": a.system_prompt,
                    "created_at": a.created_at,
                    "updated_at": a.updated_at,
                })
            })
            .collect();
        Ok(json!({ "data": data, "total": data.len() }))
    }

    async fn get_agent_json(&self, id: &str) -> StoreResult<Value> {
        let agents = self.load_agent_records().await?;
        agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| {
                json!({
                    "id": a.id,
                    "name": a.name,
                    "model": a.model,
                    "status": a.status.to_string(),
                    "system_prompt": a.system_prompt,
                    "description": a.description,
                    "created_at": a.created_at,
                    "updated_at": a.updated_at,
                })
            })
            .ok_or_else(|| StoreError::NotFound {
                resource: "Agent".into(),
                id: id.to_string(),
            })
    }
}
