//! `tool:init_workspace` — seed `AGENTS.md` and example skill under the operator workspace.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::paths::clawz_home;
use crate::tools::{SetupTool, ToolContext, ToolInput, ToolResult};
use crate::types::SetupArtifact;

const DEFAULT_AGENTS_MD: &str = r#"# ClawZ agent instructions

You are a helpful ClawZ assistant. Follow workspace skills when relevant.
Prefer concise answers with clear next steps.
"#;

const EXAMPLE_SKILL_MD: &str = r#"# Code review

When reviewing code, check correctness, security, and tests.
Cite file paths and suggest minimal diffs.
"#;

/// Seeds `~/.clawz/workspace` (or `CLAWZ_WORKSPACE`) with starter files.
#[derive(Debug, Clone, Copy, Default)]
pub struct InitWorkspaceTool;

impl SetupTool for InitWorkspaceTool {
    fn name(&self) -> &'static str {
        "init_workspace"
    }

    fn requires_confirm(&self) -> bool {
        false
    }

    fn execute(&self, _ctx: &ToolContext, _input: &ToolInput) -> Result<ToolResult> {
        let root = workspace_root();
        let created = init_workspace_at(&root)?;

        let mut artifacts = vec![SetupArtifact::AgentsMd {
            path: root.join("AGENTS.md").display().to_string(),
        }];
        for path in created.skills {
            artifacts.push(SetupArtifact::AgentsMd { path });
        }

        Ok(ToolResult {
            ok: true,
            message: format!("workspace ready at {}", root.display()),
            artifacts,
        })
    }
}

/// Resolved workspace directory (`CLAWZ_WORKSPACE` or `~/.clawz/workspace`).
pub fn workspace_root() -> PathBuf {
    std::env::var("CLAWZ_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| clawz_home().join("workspace"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitWorkspaceReport {
    pub root: PathBuf,
    pub agents_md_created: bool,
    pub skill_created: bool,
    pub skills: Vec<String>,
}

/// Idempotent workspace seed (mirrors `scripts/init-workspace.sh`).
pub fn init_workspace_at(root: &Path) -> Result<InitWorkspaceReport> {
    fs::create_dir_all(root.join("skills/code-review"))?;

    let agents_path = root.join("AGENTS.md");
    let agents_md_created = if agents_path.exists() {
        false
    } else {
        fs::write(&agents_path, DEFAULT_AGENTS_MD)?;
        true
    };

    let skill_path = root.join("skills/code-review/SKILL.md");
    let skill_created = if skill_path.exists() {
        false
    } else {
        fs::write(&skill_path, EXAMPLE_SKILL_MD)?;
        true
    };

    let mut skills = Vec::new();
    if skill_created {
        skills.push(skill_path.display().to_string());
    }

    Ok(InitWorkspaceReport {
        root: root.to_path_buf(),
        agents_md_created,
        skill_created,
        skills,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_workspace_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("workspace");
        let first = init_workspace_at(&root).unwrap();
        assert!(first.agents_md_created);
        assert!(first.skill_created);

        let second = init_workspace_at(&root).unwrap();
        assert!(!second.agents_md_created);
        assert!(!second.skill_created);
        assert!(root.join("AGENTS.md").exists());
        assert!(root.join("skills/code-review/SKILL.md").exists());
    }

}
