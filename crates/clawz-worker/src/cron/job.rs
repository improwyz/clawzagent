//! Cron job types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Optional delivery of cron output to a channel adapter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronDelivery {
    pub channel_type: String,
    #[serde(default)]
    pub config: Value,
    pub to: String,
}

/// Scheduled agent job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    /// Cron expression (5-field `min hour dom month dow` or 6-field with seconds).
    pub cron_expr: String,
    pub prompt: String,
    pub agent_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<CronDelivery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_toolsets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

impl CronJob {
    pub fn new(
        cron_expr: impl Into<String>,
        prompt: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            cron_expr: cron_expr.into(),
            prompt: prompt.into(),
            agent_id: agent_id.into(),
            enabled: true,
            delivery: None,
            disabled_toolsets: Vec::new(),
            last_run_at: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Request body for creating a job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCronJobRequest {
    pub cron_expr: String,
    pub prompt: String,
    pub agent_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub delivery: Option<CronDelivery>,
    #[serde(default)]
    pub disabled_toolsets: Vec<String>,
}

/// Result of a manual or scheduled cron run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRunResult {
    pub job_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub content: String,
    #[serde(default)]
    pub delivered: bool,
}
