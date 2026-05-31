//! `tool:write_env` — patch `.env` key=value pairs (confirm-gated).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Result, SetupError};
use crate::tools::{SetupTool, ToolContext, ToolInput, ToolResult};
use crate::types::SetupArtifact;

/// Patches operator `.env` with explicit key/value pairs.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteEnvTool;

#[derive(Debug, Deserialize)]
struct WriteEnvArgs {
    #[serde(flatten)]
    pairs: BTreeMap<String, String>,
}

impl SetupTool for WriteEnvTool {
    fn name(&self) -> &'static str {
        "write_env"
    }

    fn requires_confirm(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &ToolContext, input: &ToolInput) -> Result<ToolResult> {
        let pairs = parse_pairs(&input.args)?;
        if pairs.is_empty() {
            return Err(SetupError::InvalidTransition(
                "write_env requires at least one key".into(),
            ));
        }

        let keys: Vec<String> = pairs.keys().cloned().collect();
        patch_env_file(&ctx.env_path, &pairs)?;

        Ok(ToolResult {
            ok: true,
            message: format!(
                "patched {} key(s) in {}",
                keys.len(),
                ctx.env_path.display()
            ),
            artifacts: vec![SetupArtifact::EnvPatch { keys }],
        })
    }
}

fn parse_pairs(args: &Value) -> Result<BTreeMap<String, String>> {
    if args.is_null() || args.as_object().is_some_and(|o| o.is_empty()) {
        return Ok(BTreeMap::new());
    }
    let parsed: WriteEnvArgs = serde_json::from_value(args.clone())
        .map_err(|e| SetupError::serialization(e.to_string()))?;
    Ok(parsed.pairs)
}

/// Update or append `KEY=value` lines in an env file.
pub fn patch_env_file(path: &Path, pairs: &BTreeMap<String, String>) -> Result<()> {
    let mut lines: Vec<String> = if path.exists() {
        fs::read_to_string(path)?
            .lines()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };

    for (key, value) in pairs {
        let prefix = format!("{key}=");
        let new_line = format!("{key}={value}");
        if let Some(line) = lines
            .iter_mut()
            .find(|l| l.starts_with(&prefix) || *l == key)
        {
            *line = new_line;
        } else {
            lines.push(new_line);
        }
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let body = if lines.is_empty() {
        String::new()
    } else {
        let mut body = lines.join("\n");
        body.push('\n');
        body
    };
    fs::write(path, body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ConfirmGate, SetupToolRegistry, ToolContext};
    use crate::types::SetupPlatform;
    use crate::SetupSession;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn patch_env_updates_and_appends() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".env");
        fs::write(&path, "FOO=old\nBAR=1\n").unwrap();

        let mut pairs = BTreeMap::new();
        pairs.insert("FOO".into(), "new".into());
        pairs.insert("BAZ".into(), "2".into());
        patch_env_file(&path, &pairs).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("FOO=new"));
        assert!(content.contains("BAR=1"));
        assert!(content.contains("BAZ=2"));
    }

    #[test]
    fn write_env_requires_confirm_via_registry() {
        let dir = TempDir::new().unwrap();
        let env_path = dir.path().join(".env");
        let mut reg = SetupToolRegistry::new();
        reg.set_confirm_gate(ConfirmGate::new("confirm-123"));
        reg.register(WriteEnvTool);

        let ctx = ToolContext {
            session: SetupSession::new(SetupPlatform::Linux),
            confirm_token: Some("confirm-123".into()),
            repo_root: dir.path().to_path_buf(),
            env_path: env_path.clone(),
        };
        let input = ToolInput {
            args: json!({ "CLAWZ_IMAGE_TAG": "v1.0.0" }),
        };
        let out = reg.run("write_env", &ctx, &input).unwrap();
        assert!(out.ok);
        let content = fs::read_to_string(&env_path).unwrap();
        assert!(content.contains("CLAWZ_IMAGE_TAG=v1.0.0"));
    }
}
