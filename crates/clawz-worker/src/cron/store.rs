//! File-backed cron job store (`~/.clawz/cron/jobs.json`).

use std::path::{Path, PathBuf};

use chrono::Utc;
use tokio::fs::OpenOptions;
use tokio::sync::RwLock;

use clawz_core::error::{ClawzError, Result};

use super::job::{CreateCronJobRequest, CronJob};

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct JobFile {
    #[serde(default)]
    jobs: Vec<CronJob>,
}

/// Persistent job store for standalone deployments.
pub struct FileJobStore {
    path: PathBuf,
    lock_path: PathBuf,
    inner: RwLock<JobFile>,
}

/// Exclusive lock for one scheduler tick (multi-process safe).
pub struct CronTickLock {
    lock_path: PathBuf,
}

impl Drop for CronTickLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

impl FileJobStore {
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("CLAWZ_CRON_JOBS_FILE") {
            return PathBuf::from(p);
        }
        let home = std::env::var("CLAWZ_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".clawz")
            });
        home.join("cron/jobs.json")
    }

    pub async fn open_default() -> Result<Self> {
        Self::open(Self::default_path()).await
    }

    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ClawzError::Internal(format!("create cron dir: {e}")))?;
        }
        let file = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let lock_path = path.with_extension("lock");
        Ok(Self {
            path,
            lock_path,
            inner: RwLock::new(file),
        })
    }

    /// Acquire an exclusive tick lock; skip this tick if another process holds it.
    pub async fn try_acquire_tick_lock(&self) -> Result<CronTickLock> {
        if let Some(parent) = self.lock_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ClawzError::Internal(format!("create cron lock dir: {e}")))?;
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.lock_path)
            .await
            .map_err(|_| ClawzError::Internal("cron tick lock held".into()))?;
        Ok(CronTickLock {
            lock_path: self.lock_path.clone(),
        })
    }

    async fn persist(&self) -> Result<()> {
        let snapshot = {
            let guard = self.inner.read().await;
            guard.clone()
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ClawzError::Internal(format!("create cron dir: {e}")))?;
        }
        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;
        std::fs::write(&self.path, json)
            .map_err(|e| ClawzError::Internal(format!("write cron jobs: {e}")))?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<CronJob>> {
        Ok(self.inner.read().await.jobs.clone())
    }

    pub async fn get(&self, id: &str) -> Result<CronJob> {
        let file = self.inner.read().await;
        file.jobs
            .iter()
            .find(|j| j.id == id)
            .cloned()
            .ok_or_else(|| ClawzError::NotFound {
                entity: "CronJob".into(),
                id: id.into(),
            })
    }

    pub async fn create(&self, req: CreateCronJobRequest) -> Result<CronJob> {
        let mut job = CronJob::new(req.cron_expr, req.prompt, req.agent_id);
        if let Some(name) = req.name {
            job.name = name;
        }
        if let Some(enabled) = req.enabled {
            job.enabled = enabled;
        }
        job.delivery = req.delivery;
        job.disabled_toolsets = req.disabled_toolsets;

        let mut file = self.inner.write().await;
        file.jobs.push(job.clone());
        drop(file);
        self.persist().await?;
        Ok(job)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut file = self.inner.write().await;
        let before = file.jobs.len();
        file.jobs.retain(|j| j.id != id);
        if file.jobs.len() == before {
            return Err(ClawzError::NotFound {
                entity: "CronJob".into(),
                id: id.into(),
            });
        }
        drop(file);
        self.persist().await
    }

    pub async fn mark_run(&self, id: &str) -> Result<()> {
        let mut file = self.inner.write().await;
        let job =
            file.jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| ClawzError::NotFound {
                    entity: "CronJob".into(),
                    id: id.into(),
                })?;
        job.last_run_at = Some(Utc::now());
        job.updated_at = Utc::now();
        drop(file);
        self.persist().await
    }
}
