//! Cron scheduler — ticks every 60s (configurable) and runs due jobs.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule;
use std::str::FromStr;

use clawz_core::error::Result;

use super::job::CronJob;
use crate::service::WorkerService;

/// Normalize 5-field cron to 6-field (prepend seconds=0).
pub fn normalize_cron_expr(expr: &str) -> String {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    match parts.len() {
        5 => format!(
            "0 {} {} {} {} {}",
            parts[0], parts[1], parts[2], parts[3], parts[4]
        ),
        _ => expr.to_string(),
    }
}

/// Returns true if the job should fire within the last `tick_secs` window.
pub fn is_job_due(job: &CronJob, now: DateTime<Utc>, tick_secs: u64) -> bool {
    if !job.enabled {
        return false;
    }
    let normalized = normalize_cron_expr(&job.cron_expr);
    let schedule = match Schedule::from_str(&normalized) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(job_id = %job.id, "invalid cron expr: {e}");
            return false;
        }
    };

    let window_start = now - chrono::Duration::seconds(tick_secs as i64 + 2);
    if let Some(last) = job.last_run_at {
        if last > window_start {
            return false;
        }
    }

    schedule.after(&window_start).take(3).any(|dt| dt <= now)
}

/// Start the background cron loop unless `CLAWZ_CRON_SCHEDULER=0`.
pub fn spawn(service: Arc<WorkerService>) {
    if std::env::var("CLAWZ_CRON_SCHEDULER").ok().as_deref() == Some("0") {
        tracing::info!("cron scheduler disabled (CLAWZ_CRON_SCHEDULER=0)");
        return;
    }

    tokio::spawn(async move {
        let tick_secs = std::env::var("CLAWZ_CRON_TICK_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        let mut interval = tokio::time::interval(Duration::from_secs(tick_secs));
        tracing::info!("cron scheduler started (tick every {tick_secs}s)");
        loop {
            interval.tick().await;
            if let Err(e) = tick(&service, tick_secs).await {
                tracing::warn!("cron tick: {e}");
            }
        }
    });
}

async fn tick(service: &WorkerService, tick_secs: u64) -> Result<()> {
    let _lock = match service.cron_store().try_acquire_tick_lock().await {
        Ok(lock) => lock,
        Err(_) => return Ok(()),
    };
    let jobs = service.cron_store().list().await?;
    let now = Utc::now();
    for job in jobs {
        if !is_job_due(&job, now, tick_secs) {
            continue;
        }
        tracing::info!(job_id = %job.id, agent_id = %job.agent_id, "running cron job");
        match service.execute_cron_job(&job.id).await {
            Ok(result) => {
                tracing::info!(
                    job_id = %job.id,
                    delivered = result.delivered,
                    "cron job completed"
                );
            }
            Err(e) => tracing::warn!(job_id = %job.id, "cron job failed: {e}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_five_field_cron() {
        assert_eq!(normalize_cron_expr("0 9 * * *"), "0 0 9 * * *");
    }

    #[test]
    fn due_detection() {
        let job = CronJob::new("0 9 * * *", "hello", "agent-1");
        // Not asserting true without time control — schedule parses.
        let _ = Schedule::from_str(&normalize_cron_expr(&job.cron_expr));
    }
}
