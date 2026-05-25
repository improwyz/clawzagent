use crate::tools::tool_trait::{Tool, ToolContext};
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::{ToolResult, ToolSchema};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Executor;
use uuid::Uuid;

/// Escalation priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EscalationPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for EscalationPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl EscalationPriority {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "low" => Self::Low,
            "high" => Self::High,
            "critical" => Self::Critical,
            _ => Self::Medium,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationEvent {
    pub id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub reason: String,
    pub priority: EscalationPriority,
    pub context: Value,
    pub created_at: String,
    pub task_paused: bool,
}

pub struct EscalateTool;

impl EscalateTool {
    pub fn new() -> Self {
        Self
    }

    /// Persist the escalation event. Uses DB if available, otherwise logs it.
    async fn persist_escalation(event: &EscalationEvent) -> Result<(), ClawzError> {
        // Try to write to DB
        if let Ok(db_url) = std::env::var("CLAWZ_DB_URL").or_else(|_| std::env::var("DATABASE_URL")) {
            let pool = sqlx::PgPool::connect(&db_url)
                .await
                .map_err(|e| ClawzError::Database(format!("DB connect failed: {e}")))?;

            let context_json = serde_json::to_string(&event.context)
                .unwrap_or_else(|_| "null".into());

            sqlx::query(
                r#"
                INSERT INTO escalation_events
                    (id, agent_id, conversation_id, reason, priority, context, created_at, task_paused)
                VALUES
                    ($1::uuid, $2, $3, $4, $5, $6::jsonb, $7, $8)
                ON CONFLICT (id) DO NOTHING
                "#,
            )
            .bind(&event.id)
            .bind(&event.agent_id)
            .bind(&event.conversation_id)
            .bind(&event.reason)
            .bind(event.priority.to_string())
            .bind(context_json)
            .bind(&event.created_at)
            .bind(event.task_paused)
            .execute(&pool)
            .await
            .map_err(|e| ClawzError::Database(format!("escalation insert failed: {e}")))?;

            pool.close().await;
        } else {
            // Fall back to structured log
            log::warn!(
                "[ESCALATION] id={} agent={} priority={} reason={}",
                event.id,
                event.agent_id,
                event.priority,
                event.reason
            );
        }

        // Send webhook notification if configured
        if let Ok(webhook_url) = std::env::var("CLAWZ_ESCALATION_WEBHOOK") {
            send_webhook_notification(&webhook_url, event).await.ok();
        }

        Ok(())
    }
}

async fn send_webhook_notification(
    webhook_url: &str,
    event: &EscalationEvent,
) -> Result<(), ClawzError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ClawzError::Tool(format!("HTTP client failed: {e}")))?;

    let payload = serde_json::json!({
        "event_type": "escalation",
        "escalation": event
    });

    client
        .post(webhook_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| ClawzError::Tool(format!("webhook delivery failed: {e}")))?;

    Ok(())
}

impl Default for EscalateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for EscalateTool {
    fn name(&self) -> &str {
        "escalate"
    }

    fn description(&self) -> &str {
        "Mark the current task as needing human intervention. Records the escalation event with priority and reason, and optionally pauses execution."
    }


    fn primitive(&self) -> ActionPrimitive { ActionPrimitive::Notify }
    fn risk(&self) -> RiskLevel { RiskLevel::Medium }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "escalate".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "Explanation of why human intervention is needed"
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["low", "medium", "high", "critical"],
                        "description": "Urgency level (default: medium)"
                    },
                    "context": {
                        "type": "object",
                        "description": "Additional context/data to include with the escalation"
                    },
                    "pause_task": {
                        "type": "boolean",
                        "description": "Whether to pause the current task (default: true)"
                    }
                },
                "required": ["reason"]
            }),
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: Value,
    ) -> Result<ToolResult, ClawzError> {
        let reason = args["reason"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("reason required".into()))?;

        let priority_str = args["priority"].as_str().unwrap_or("medium");
        let priority = EscalationPriority::from_str(priority_str);
        let pause_task = args["pause_task"].as_bool().unwrap_or(true);
        let context = args["context"].clone();

        let event = EscalationEvent {
            id: Uuid::new_v4().to_string(),
            agent_id: ctx.agent_id.clone(),
            conversation_id: ctx.conversation_id.clone(),
            reason: reason.to_string(),
            priority,
            context,
            created_at: Utc::now().to_rfc3339(),
            task_paused: pause_task,
        };

        Self::persist_escalation(&event).await?;

        let output = serde_json::json!({
            "escalated": true,
            "id": event.id,
            "priority": priority.to_string(),
            "reason": reason,
            "task_paused": pause_task,
            "created_at": event.created_at,
            "message": format!(
                "Escalation created ({}): {}. Human operator notified.",
                priority, reason
            )
        })
        .to_string();

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolConfig;

    fn make_ctx() -> ToolContext {
        ToolContext {
            agent_id: "agent-123".into(),
            conversation_id: "conv-456".into(),
            user_id: Some("user-789".into()),
            config: ToolConfig::default(),
        }
    }

    #[test]
    fn test_escalate_name() {
        assert_eq!(EscalateTool::new().name(), "escalate");
    }

    #[test]
    fn test_escalate_schema() {
        let schema = EscalateTool::new().schema();
        assert_eq!(schema.name, "escalate");
        assert!(schema.parameters["properties"]["reason"].is_object());
        assert!(schema.parameters["properties"]["priority"].is_object());
    }

    #[tokio::test]
    async fn test_escalate_missing_reason() {
        let tool = EscalateTool::new();
        let ctx = make_ctx();
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_escalate_success_no_db() {
        // Without DB, escalation should still succeed (logs instead)
        let tool = EscalateTool::new();
        let ctx = make_ctx();
        // Only test when DB is not configured
        if std::env::var("CLAWZ_DB_URL").is_err() && std::env::var("DATABASE_URL").is_err() {
            let result = tool
                .execute(
                    &ctx,
                    serde_json::json!({
                        "reason": "Test escalation",
                        "priority": "high",
                        "pause_task": false
                    }),
                )
                .await
                .unwrap();
            assert!(!result.is_error);
            let parsed: Value = serde_json::from_str(&result.output).unwrap();
            assert_eq!(parsed["escalated"], true);
            assert_eq!(parsed["priority"], "high");
        }
    }

    #[test]
    fn test_priority_from_str() {
        assert_eq!(EscalationPriority::from_str("critical"), EscalationPriority::Critical);
        assert_eq!(EscalationPriority::from_str("low"), EscalationPriority::Low);
        assert_eq!(EscalationPriority::from_str("unknown"), EscalationPriority::Medium);
    }
}
