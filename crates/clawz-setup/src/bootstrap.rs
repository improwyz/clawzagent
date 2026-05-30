//! Agent identity bootstrap — system prompt, `AGENTS.md`, and gateway payload.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Inputs collected during wizard phase 6 (agent identity).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityInput {
    pub name: String,
    pub who_am_i: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Generated persona artifacts for workspace + runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityArtifacts {
    pub system_prompt: String,
    pub agents_md: String,
}

/// JSON payload for `POST /agents` (standalone default agent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentConfigPayload {
    pub id: Uuid,
    pub name: String,
    pub model: String,
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Builds agent identity content for onboarding.
#[derive(Debug, Clone, Copy, Default)]
pub struct AgentBootstrap;

impl AgentBootstrap {
    /// Compose `system_prompt` and workspace `AGENTS.md` from wizard answers.
    pub fn build_identity(input: &IdentityInput) -> IdentityArtifacts {
        let system_prompt = format_system_prompt(&input.who_am_i, &input.role);
        let agents_md = format_agents_md(&input.name, &input.who_am_i, &input.role);
        IdentityArtifacts {
            system_prompt,
            agents_md,
        }
    }

    /// Single-agent config for gateway create (standalone path).
    pub fn agent_config(
        input: &IdentityInput,
        artifacts: &IdentityArtifacts,
    ) -> AgentConfigPayload {
        let model = input
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-5".into());
        let mut metadata = HashMap::new();
        metadata.insert(
            "who_am_i".into(),
            serde_json::Value::String(input.who_am_i.clone()),
        );
        metadata.insert(
            "onboarding".into(),
            serde_json::Value::Bool(true),
        );

        AgentConfigPayload {
            id: Uuid::new_v4(),
            name: input.name.clone(),
            model,
            system_prompt: artifacts.system_prompt.clone(),
            tools: vec![],
            temperature: Some(0.7),
            max_tokens: None,
            metadata,
        }
    }
}

fn format_system_prompt(who_am_i: &str, role: &str) -> String {
    format!(
        "You are {who_am_i}.\n\n## Role\n{role}\n\nFollow workspace AGENTS.md and skills when relevant."
    )
}

fn format_agents_md(name: &str, who_am_i: &str, role: &str) -> String {
    format!(
        r#"# {name}

## Who I am
{who_am_i}

## Role
{role}

## Operating guidelines
- Prefer concise, actionable answers.
- Use workspace skills when they match the task.
- Ask clarifying questions when requirements are ambiguous.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_input() -> IdentityInput {
        IdentityInput {
            name: "Ops".into(),
            who_am_i: "a ClawZ operations assistant".into(),
            role: "Help the operator install, monitor, and troubleshoot ClawZ.".into(),
            model: None,
        }
    }

    #[test]
    fn build_identity_includes_who_and_role() {
        let artifacts = AgentBootstrap::build_identity(&sample_input());
        assert!(artifacts.system_prompt.contains("operations assistant"));
        assert!(artifacts.system_prompt.contains("install, monitor"));
        assert!(artifacts.agents_md.contains("# Ops"));
        assert!(artifacts.agents_md.contains("Who I am"));
    }

    #[test]
    fn agent_config_serializes_for_gateway() {
        let input = sample_input();
        let artifacts = AgentBootstrap::build_identity(&input);
        let cfg = AgentBootstrap::agent_config(&input, &artifacts);
        assert_eq!(cfg.name, "Ops");
        assert_eq!(cfg.model, "claude-sonnet-4-5");
        assert!(!cfg.system_prompt.is_empty());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"system_prompt\""));
    }
}
