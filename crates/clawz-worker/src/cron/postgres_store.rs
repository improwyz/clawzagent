//! Postgres-backed cron job store for micro/elastic deployments.
//!
//! Falls back to [`FileJobStore`] when the `clawz_cron_jobs` table is not present.

use chrono::Utc;
use sqlx::PgPool;

use clawz_core::error::{ClawzError, Result};

use super::job::{CreateCronJobRequest, CronJob};
use super::store::{CronTickLock, FileJobStore};

/// Enterprise cron persistence with automatic file fallback.
pub struct PostgresJobStore {
    pool: PgPool,
    fallback: FileJobStore,
}

impl PostgresJobStore {
    pub async fn open(pool: PgPool) -> Result<Self> {
        let fallback = FileJobStore::open_default().await?;
        Ok(Self { pool, fallback })
    }

    async fn table_ready(&self) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'clawz_cron_jobs'
            )",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }

    fn job_from_create(req: CreateCronJobRequest) -> CronJob {
        let mut job = CronJob::new(req.cron_expr, req.prompt, req.agent_id);
        if let Some(name) = req.name {
            job.name = name;
        }
        if let Some(enabled) = req.enabled {
            job.enabled = enabled;
        }
        job.delivery = req.delivery;
        job.disabled_toolsets = req.disabled_toolsets;
        job
    }
}

impl PostgresJobStore {
    pub async fn list(&self) -> Result<Vec<CronJob>> {
        if !self.table_ready().await {
            return self.fallback.list().await;
        }
        let rows = sqlx::query_as::<_, CronJobRow>(
            "SELECT id, name, cron_expr, prompt, agent_id, enabled, delivery, disabled_toolsets,
                    last_run_at, created_at, updated_at
             FROM clawz_cron_jobs ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get(&self, id: &str) -> Result<CronJob> {
        if !self.table_ready().await {
            return self.fallback.get(id).await;
        }
        let row = sqlx::query_as::<_, CronJobRow>(
            "SELECT id, name, cron_expr, prompt, agent_id, enabled, delivery, disabled_toolsets,
                    last_run_at, created_at, updated_at
             FROM clawz_cron_jobs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?
        .ok_or_else(|| ClawzError::NotFound {
            entity: "CronJob".into(),
            id: id.into(),
        })?;
        Ok(row.into())
    }

    pub async fn create(&self, req: CreateCronJobRequest) -> Result<CronJob> {
        if !self.table_ready().await {
            return self.fallback.create(req).await;
        }
        let job = Self::job_from_create(req);
        sqlx::query(
            "INSERT INTO clawz_cron_jobs
                (id, name, cron_expr, prompt, agent_id, enabled, delivery, disabled_toolsets, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&job.id)
        .bind(&job.name)
        .bind(&job.cron_expr)
        .bind(&job.prompt)
        .bind(&job.agent_id)
        .bind(job.enabled)
        .bind(serde_json::to_value(&job.delivery).ok())
        .bind(&job.disabled_toolsets)
        .bind(job.created_at)
        .bind(job.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;
        Ok(job)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        if !self.table_ready().await {
            return self.fallback.delete(id).await;
        }
        let result = sqlx::query("DELETE FROM clawz_cron_jobs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| ClawzError::Database(e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err(ClawzError::NotFound {
                entity: "CronJob".into(),
                id: id.into(),
            });
        }
        Ok(())
    }

    pub async fn mark_run(&self, id: &str) -> Result<()> {
        if !self.table_ready().await {
            return self.fallback.mark_run(id).await;
        }
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE clawz_cron_jobs SET last_run_at = $2, updated_at = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| ClawzError::Database(e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err(ClawzError::NotFound {
                entity: "CronJob".into(),
                id: id.into(),
            });
        }
        Ok(())
    }

    pub async fn try_acquire_tick_lock(&self) -> Result<CronTickLock> {
        self.fallback.try_acquire_tick_lock().await
    }
}

#[derive(Debug, sqlx::FromRow)]
struct CronJobRow {
    id: String,
    name: String,
    cron_expr: String,
    prompt: String,
    agent_id: String,
    enabled: bool,
    delivery: Option<serde_json::Value>,
    disabled_toolsets: Vec<String>,
    last_run_at: Option<chrono::DateTime<Utc>>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

impl From<CronJobRow> for CronJob {
    fn from(row: CronJobRow) -> Self {
        CronJob {
            id: row.id,
            name: row.name,
            cron_expr: row.cron_expr,
            prompt: row.prompt,
            agent_id: row.agent_id,
            enabled: row.enabled,
            delivery: row.delivery.and_then(|v| serde_json::from_value(v).ok()),
            disabled_toolsets: row.disabled_toolsets,
            last_run_at: row.last_run_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}
