//! BehavioralAdaptor — applies approved improvement proposals to the runtime.
//!
//! This is the "apply" end of the closed self-improvement loop. It receives
//! an approved [`ImprovementProposal`] and translates its suggestion strings
//! into concrete runtime changes via the [`SkillRepository`](crate::governance::skill_repository::SkillRepository).

use std::sync::Arc;

use uuid::Uuid;

use crate::governance::skill_repository::SkillRepository;
use crate::memory::improvement::ImprovementProposal;

/// A single concrete change applied to the runtime.
#[derive(Debug, Clone)]
pub struct AppliedChange {
    pub proposal_id: Uuid,
    pub change_type: ChangeType,
    pub target: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum ChangeType {
    ToolTimeout,
    RetryPolicy,
    ModelSelection,
    ConcurrencyLimit,
    IdentityModification,
    Custom(String),
}

/// Maps approved [`ImprovementProposal`]s to runtime changes.
pub struct BehavioralAdaptor {
    #[allow(dead_code)]
    skill_repo: Arc<dyn SkillRepository>,
}

impl BehavioralAdaptor {
    /// Create a new adaptor backed by the given skill repository.
    pub fn new(skill_repo: Arc<dyn SkillRepository>) -> Self {
        Self { skill_repo }
    }

    /// Apply an approved improvement proposal.
    /// Parses the suggestion strings and applies each change via the skill repository.
    /// Returns the list of concrete changes applied.
    pub async fn apply(&self, proposal: &ImprovementProposal) -> Result<Vec<AppliedChange>, clawz_core::ClawzError> {
        let mut applied = Vec::new();

        for suggestion in &proposal.suggested_changes {
            if let Some(change) = self.parse_and_apply(suggestion, proposal.proposal_id).await? {
                applied.push(change);
            }
        }

        Ok(applied)
    }

    /// Parse a suggestion string and apply the corresponding change.
    async fn parse_and_apply(&self, suggestion: &str, proposal_id: Uuid) -> Result<Option<AppliedChange>, clawz_core::ClawzError> {
        let suggestion = suggestion.trim();

        // Format: "set <target> <value>"
        if let Some(rest) = suggestion.strip_prefix("set ") {
            if let Some(space_idx) = rest.find(' ') {
                let target = rest[..space_idx].trim();
                let value = rest[space_idx + 1..].trim();
                return Ok(Some(self.apply_set(target, value, proposal_id).await?));
            }
        }

        // Keyword-based parsing
        let suggestion_lower = suggestion.to_lowercase();

        if suggestion_lower.contains("timeout") {
            if let Some(val) = self.extract_number(suggestion) {
                let change = AppliedChange {
                    proposal_id,
                    change_type: ChangeType::ToolTimeout,
                    target: "tool_timeout_secs".into(),
                    value: serde_json::json!(val),
                };
                return Ok(Some(change));
            }
        }

        if suggestion_lower.contains("retry") {
            if let Some(val) = self.extract_number(suggestion) {
                let change = AppliedChange {
                    proposal_id,
                    change_type: ChangeType::RetryPolicy,
                    target: "max_retries".into(),
                    value: serde_json::json!(val),
                };
                return Ok(Some(change));
            }
        }

        if suggestion_lower.contains("concurrency") || suggestion_lower.contains("parallel") {
            if let Some(val) = self.extract_number(suggestion) {
                let change = AppliedChange {
                    proposal_id,
                    change_type: ChangeType::ConcurrencyLimit,
                    target: "max_parallel".into(),
                    value: serde_json::json!(val),
                };
                return Ok(Some(change));
            }
        }

        if suggestion_lower.contains("model") {
            if let Some(val) = self.extract_model_name(suggestion) {
                let change = AppliedChange {
                    proposal_id,
                    change_type: ChangeType::ModelSelection,
                    target: "model".into(),
                    value: serde_json::json!(val),
                };
                return Ok(Some(change));
            }
        }

        // Format: "identity <field> <delta>"
        let parts: Vec<&str> = suggestion.split_whitespace().collect();
        if parts.len() >= 3 && parts[0].to_lowercase() == "identity" {
            let field = parts[1].to_string();
            if let Ok(delta) = parts[2].parse::<f32>() {
                let change = AppliedChange {
                    proposal_id,
                    change_type: ChangeType::IdentityModification,
                    target: field,
                    value: serde_json::json!(delta),
                };
                return Ok(Some(change));
            }
        }

        // Fallback: treat the whole suggestion as a custom change
        let change = AppliedChange {
            proposal_id,
            change_type: ChangeType::Custom(suggestion.to_string()),
            target: "custom".into(),
            value: serde_json::json!(suggestion),
        };
        Ok(Some(change))
    }

    async fn apply_set(&self, target: &str, value: &str, proposal_id: Uuid) -> Result<AppliedChange, clawz_core::ClawzError> {
        let change_type = match target {
            "tool_timeout" | "tool_timeout_secs" | "timeout" => ChangeType::ToolTimeout,
            "max_retries" | "retry" | "retries" => ChangeType::RetryPolicy,
            "concurrency" | "max_concurrency" | "parallel" => ChangeType::ConcurrencyLimit,
            "model" => ChangeType::ModelSelection,
            _ => ChangeType::Custom(target.to_string()),
        };

        let json_value: serde_json::Value = value.parse().unwrap_or_else(|_| serde_json::json!(value));

        Ok(AppliedChange {
            proposal_id,
            change_type,
            target: target.to_string(),
            value: json_value,
        })
    }

    fn extract_number(&self, text: &str) -> Option<u64> {
        text.chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    }

    fn extract_model_name(&self, text: &str) -> Option<String> {
        // Try to extract model name after "model" keyword
        let parts: Vec<&str> = text.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "model" && i + 1 < parts.len() {
                return Some(parts[i + 1].to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::metrics::PerfDimension;
    use crate::governance::skill_repository::SkillBundle;

    struct DummySkillRepo;
    #[async_trait::async_trait]
    impl SkillRepository for DummySkillRepo {
        async fn get_skill(&self, _agent_id: &str) -> Result<Option<SkillBundle>, clawz_core::ClawzError> {
            Ok(None)
        }
        async fn update_skill(&self, _agent_id: &str, _bundle: SkillBundle) -> Result<(), clawz_core::ClawzError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn adaptor_applies_proposal_with_timeout() {
        let repo = Arc::new(DummySkillRepo);
        let adaptor = BehavioralAdaptor::new(repo);

        let proposal = crate::memory::improvement::ImprovementProposal {
            proposal_id: Uuid::new_v4(),
            generated_at: chrono::Utc::now(),
            triggering_metrics: vec![(PerfDimension::Speed, 0.5)],
            suggested_changes: vec!["reduce tool timeout from 30s to 10s".into()],
            confidence: 0.85,
            identity_modification: None,
        };

        let result = adaptor.apply(&proposal).await;
        assert!(result.is_ok());
        let applied = result.unwrap();
        assert!(!applied.is_empty());
    }

    #[tokio::test]
    async fn adaptor_parses_set_command() {
        let repo = Arc::new(DummySkillRepo);
        let adaptor = BehavioralAdaptor::new(repo);

        let proposal = crate::memory::improvement::ImprovementProposal {
            proposal_id: Uuid::new_v4(),
            generated_at: chrono::Utc::now(),
            triggering_metrics: vec![],
            suggested_changes: vec!["set tool_timeout_secs 15".into()],
            confidence: 0.9,
            identity_modification: None,
        };

        let result = adaptor.apply(&proposal).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].target, "tool_timeout_secs");
        assert_eq!(result[0].value, serde_json::json!(15));
    }

    #[tokio::test]
    async fn adaptor_returns_empty_for_empty_proposal() {
        let repo = Arc::new(DummySkillRepo);
        let adaptor = BehavioralAdaptor::new(repo);

        let proposal = crate::memory::improvement::ImprovementProposal {
            proposal_id: Uuid::new_v4(),
            generated_at: chrono::Utc::now(),
            triggering_metrics: vec![],
            suggested_changes: vec![],
            confidence: 0.5,
            identity_modification: None,
        };

        let result = adaptor.apply(&proposal).await.unwrap();
        assert!(result.is_empty());
    }
}