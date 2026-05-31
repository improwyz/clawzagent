//! `tool:install_deps` — host Docker/curl/git bootstrap.

use serde_json::Value;

use crate::deps::DependencyInstaller;
use crate::host_exec::HostScriptRunner;
use crate::tools::{SetupTool, ToolContext, ToolInput, ToolResult};

#[derive(Debug, Clone, Copy, Default)]
pub struct InstallDepsTool;

impl SetupTool for InstallDepsTool {
    fn name(&self) -> &'static str {
        "install_deps"
    }

    fn requires_confirm(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &ToolContext, input: &ToolInput) -> crate::error::Result<ToolResult> {
        let dry_run = input
            .args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let with_web = input
            .args
            .get("with_web")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let runner = HostScriptRunner::new(ctx.repo_root.clone());
        let installer = DependencyInstaller::new(runner);
        let outputs = installer.install_all(with_web, dry_run)?;

        let message = outputs
            .iter()
            .map(|o| o.command.clone())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolResult {
            ok: true,
            message,
            artifacts: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ConfirmGate, SetupToolRegistry};
    use crate::types::{SetupPlatform, SetupSession};

    #[test]
    fn install_deps_dry_run_with_confirm() {
        let root = crate::host_exec::resolve_repo_root().unwrap();
        let mut reg = SetupToolRegistry::new();
        reg.set_confirm_gate(ConfirmGate::new("yes-install"));
        reg.register(InstallDepsTool);
        let ctx = crate::tools::ToolContext {
            session: SetupSession::new(SetupPlatform::Linux),
            confirm_token: Some("yes-install".into()),
            repo_root: root,
            env_path: std::path::PathBuf::from("/tmp/.env"),
        };
        let input = ToolInput {
            args: serde_json::json!({ "dry_run": true }),
        };
        let out = reg.run("install_deps", &ctx, &input).unwrap();
        assert!(out.ok);
        assert!(out.message.contains("ensure_"));
    }
}
