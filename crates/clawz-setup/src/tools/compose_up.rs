//! `tool:compose_up` — start micro stack via install-common.

use serde_json::Value;

use crate::host_exec::HostScriptRunner;
use crate::spec::HostSpecChecker;
use crate::stack::{StackAction, StackRunner};
use crate::tools::{SetupTool, ToolContext, ToolInput, ToolResult};
use crate::types::{DeploymentChoice, InstallStrategy};

#[derive(Debug, Clone, Copy, Default)]
pub struct ComposeUpTool;

impl SetupTool for ComposeUpTool {
    fn name(&self) -> &'static str {
        "compose_up"
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
        let strategy = parse_strategy(&input.args).unwrap_or(InstallStrategy::Prebuilt);
        let deployment = ctx.session.deployment.unwrap_or(DeploymentChoice::Micro);

        let spec = HostSpecChecker::collect();
        let plan = StackRunner::plan(deployment, strategy, &spec, None)?;
        let runner = HostScriptRunner::new(ctx.repo_root.clone());
        let stack = StackRunner::new(runner);
        let out = stack.run(StackAction::Up, &plan, with_web, dry_run)?;

        Ok(ToolResult {
            ok: true,
            message: if dry_run {
                format!("{}\n{}", out.command, StackRunner::compose_argv_hint(&plan))
            } else {
                format!("Stack started ({})", StackRunner::compose_argv_hint(&plan))
            },
            artifacts: vec![],
        })
    }
}

fn parse_strategy(args: &Value) -> Option<InstallStrategy> {
    match args.get("install_strategy")?.as_str()? {
        "prebuilt" => Some(InstallStrategy::Prebuilt),
        "build" => Some(InstallStrategy::Build),
        "source" => Some(InstallStrategy::Source),
        _ => None,
    }
}
