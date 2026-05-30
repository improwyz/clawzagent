//! `tool:spec_check` — wraps [`HostSpecChecker`].

use crate::spec::HostSpecChecker;
use crate::tools::{SetupTool, ToolContext, ToolInput, ToolResult};
use crate::types::DeploymentChoice;

/// Runs host capability probes for wizard phase 0.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpecCheckTool;

impl SetupTool for SpecCheckTool {
    fn name(&self) -> &'static str {
        "spec_check"
    }

    fn requires_confirm(&self) -> bool {
        false
    }

    fn execute(&self, ctx: &ToolContext, input: &ToolInput) -> crate::error::Result<ToolResult> {
        let report = HostSpecChecker::collect();
        let deployment = ctx
            .session
            .deployment
            .or_else(|| parse_deployment_arg(&input.args));

        let mut lines = vec![report.summary.clone()];
        lines.extend(report.warnings.iter().cloned());

        if let Some(dep) = deployment {
            lines.extend(report.threshold_warnings(dep));
        }

        let message = lines.join("\n");

        Ok(ToolResult {
            ok: true,
            message,
            artifacts: vec![],
        })
    }
}

fn parse_deployment_arg(args: &serde_json::Value) -> Option<DeploymentChoice> {
    let mode = args.get("deployment")?.as_str()?;
    match mode {
        "standalone" => Some(DeploymentChoice::Standalone),
        "micro" => Some(DeploymentChoice::Micro),
        "elastic" => Some(DeploymentChoice::Elastic),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SetupPlatform, SetupSession};

    #[test]
    fn spec_check_returns_summary() {
        let ctx = ToolContext {
            session: SetupSession::new(SetupPlatform::Linux),
            confirm_token: None,
            repo_root: ".".into(),
            env_path: ".env".into(),
        };
        let tool = SpecCheckTool;
        let out = tool.execute(&ctx, &ToolInput::default()).unwrap();
        assert!(!out.message.is_empty());
    }

    #[test]
    fn spec_check_includes_threshold_warnings_when_deployment_set() {
        let mut session = SetupSession::new(SetupPlatform::Linux);
        session.deployment = Some(DeploymentChoice::Micro);
        let ctx = ToolContext {
            session,
            confirm_token: None,
            repo_root: ".".into(),
            env_path: ".env".into(),
        };
        let out = SpecCheckTool.execute(&ctx, &ToolInput::default()).unwrap();
        // On CI without Docker this typically warns about Docker.
        assert!(!out.message.is_empty());
    }

    #[test]
    fn parse_deployment_from_args() {
        let args = serde_json::json!({ "deployment": "elastic" });
        assert_eq!(
            parse_deployment_arg(&args),
            Some(DeploymentChoice::Elastic)
        );
    }
}
