//! Load `AGENTS.md`, `SOUL.md`, and `skills/*/SKILL.md` from the operator workspace.

use std::path::{Path, PathBuf};

use clawz_core::error::{ClawzError, Result};

/// One skill directory under `skills/{name}/SKILL.md`.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub name: String,
    pub path: PathBuf,
    pub content: String,
    pub description: Option<String>,
}

/// Snapshot of workspace files used to build the system prompt.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSnapshot {
    pub root: PathBuf,
    pub agents_md: Option<String>,
    pub soul_md: Option<String>,
    pub skills: Vec<SkillEntry>,
}

/// Filesystem workspace loader (`CLAWZ_HOME/workspace` or `CLAWZ_WORKSPACE`).
#[derive(Debug, Clone)]
pub struct WorkspaceLoader {
    root: PathBuf,
}

impl WorkspaceLoader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn default_home() -> Self {
        let root = std::env::var("CLAWZ_WORKSPACE").unwrap_or_else(|_| {
            let home = std::env::var("CLAWZ_HOME").unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| format!("{h}/.clawz"))
                    .unwrap_or_else(|_| "/tmp/clawz".to_string())
            });
            format!("{home}/workspace")
        });
        Self::new(root)
    }

    pub fn load_snapshot(&self) -> Result<WorkspaceSnapshot> {
        let mut snap = WorkspaceSnapshot {
            root: self.root.clone(),
            ..Default::default()
        };

        snap.agents_md = read_optional_file(&self.root.join("AGENTS.md"))?;
        snap.soul_md = read_optional_file(&self.root.join("SOUL.md"))?;
        snap.skills = load_skill_entries(&self.root.join("skills"))?;
        Ok(snap)
    }

    /// Format workspace content for injection into the system prompt.
    pub fn build_skills_prompt_snapshot(snap: &WorkspaceSnapshot) -> String {
        let mut out = String::new();

        if let Some(agents) = &snap.agents_md {
            if !agents.trim().is_empty() {
                out.push_str("\n\n## Workspace (AGENTS.md)\n");
                out.push_str(agents.trim());
            }
        }

        if let Some(soul) = &snap.soul_md {
            if !soul.trim().is_empty() {
                out.push_str("\n\n## Soul (SOUL.md)\n");
                out.push_str(soul.trim());
            }
        }

        if !snap.skills.is_empty() {
            out.push_str("\n\n## Active skills\n");
            for skill in &snap.skills {
                out.push_str(&format!("\n### {}\n", skill.name));
                if let Some(desc) = &skill.description {
                    out.push_str(desc);
                    out.push('\n');
                }
                let excerpt = truncate_preview(skill.content.trim(), 1200);
                out.push_str(&excerpt);
                out.push('\n');
            }
        }

        out
    }

    pub fn list_skill_names(&self) -> Result<Vec<String>> {
        Ok(self
            .load_snapshot()?
            .skills
            .iter()
            .map(|s| s.name.clone())
            .collect())
    }
}

fn read_optional_file(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|e| ClawzError::Internal(format!("read {}: {e}", path.display())))
}

fn load_skill_entries(skills_dir: &Path) -> Result<Vec<SkillEntry>> {
    if !skills_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(skills_dir)
        .map_err(|e| ClawzError::Internal(format!("read skills dir: {e}")))?;

    for item in read_dir {
        let item = item.map_err(|e| ClawzError::Internal(format!("skills dir entry: {e}")))?;
        let path = item.path();
        if !path.is_dir() {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        let alt = path.join("skill.md");
        let skill_path = if skill_file.exists() {
            skill_file
        } else if alt.exists() {
            alt
        } else {
            continue;
        };

        let content = std::fs::read_to_string(&skill_path)
            .map_err(|e| ClawzError::Internal(format!("read {}: {e}", skill_path.display())))?;
        let description = parse_skill_description(&content);

        entries.push(SkillEntry {
            name,
            path: skill_path,
            content,
            description,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Truncate text for prompt previews without splitting multibyte characters.
fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    match truncated.rfind(' ') {
        Some(pos) if pos > max_chars / 2 => format!("{}…", &truncated[..pos]),
        _ => format!("{truncated}…"),
    }
}

fn parse_skill_description(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.is_empty() {
            return Some(truncate_preview(trimmed, 200));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn loads_agents_and_skills() {
        let dir = std::env::temp_dir().join(format!("clawz-ws-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("skills/demo-skill")).unwrap();
        fs::write(dir.join("AGENTS.md"), "You are ClawZ.").unwrap();
        fs::write(
            dir.join("skills/demo-skill/SKILL.md"),
            "# Demo\n\nAlways cite sources.\n",
        )
        .unwrap();

        let loader = WorkspaceLoader::new(&dir);
        let snap = loader.load_snapshot().unwrap();
        assert!(snap.agents_md.as_ref().unwrap().contains("ClawZ"));
        assert_eq!(snap.skills.len(), 1);
        assert_eq!(snap.skills[0].name, "demo-skill");

        let prompt = WorkspaceLoader::build_skills_prompt_snapshot(&snap);
        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("demo-skill"));
        let _ = fs::remove_dir_all(&dir);
    }
}
