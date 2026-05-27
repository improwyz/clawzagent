//! Database pool initialization and migrations.

use clawz_core::db;
use sqlx::PgPool;

/// Connect to Postgres and run idempotent schema migrations.
pub async fn connect_and_migrate(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPool::connect(database_url).await?;
    db::run_migrations(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("migration failed: {e}"))?;
    tracing::info!("database migrations applied");
    Ok(pool)
}
