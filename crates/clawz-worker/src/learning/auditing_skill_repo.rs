//! Skill repository wrapper that records tamper-evident audit entries on updates.

use std::sync::Arc;

use async_trait::async_trait;

use crate::governance::audit::{AuditLogger, AuditResult};
use crate::governance::skill_repository::{SkillBundle, SkillRepository};

/// Delegates to an inner [`SkillRepository`] and appends audit log entries on mutation.
pub struct AuditingSkillRepository {
    inner: Arc<dyn SkillRepository>,
    audit: Arc<AuditLogger>,
}

impl AuditingSkillRepository {
    pub fn new(inner: Arc<dyn SkillRepository>, audit: Arc<AuditLogger>) -> Self {
        Self { inner, audit }
    }
}

#[async_trait]
impl SkillRepository for AuditingSkillRepository {
    async fn get_skill(
        &self,
        agent_id: &str,
    ) -> Result<Option<SkillBundle>, clawz_core::ClawzError> {
        self.inner.get_skill(agent_id).await
    }

    async fn update_skill(
        &self,
        agent_id: &str,
        bundle: SkillBundle,
    ) -> Result<(), clawz_core::ClawzError> {
        self.inner.update_skill(agent_id, bundle.clone()).await?;
        self.audit.append(
            agent_id,
            "skill:update",
            AuditResult::Allow,
            serde_json::json!({
                "version": bundle.version,
                "source": format!("{:?}", bundle.source),
            }),
        );
        Ok(())
    }
}
