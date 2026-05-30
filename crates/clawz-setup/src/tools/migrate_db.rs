//! `tool:migrate_db` — apply SQL migrations via migrate-db.sh.

use serde_json::Value;

use crate::deploy::{DeployPlan, DeployPlanner};
use crate::host_exec::HostScriptRunner;
use crate::spec::HostSpecChecker;
use crate::stack::{StackAction, StackRunner};
use crate::tools::{SetupTool, ToolContext, ToolInput, ToolResult};
use crate::types::{DeploymentChoice, InstallStrategy};

#[derive(Debug, Clone, Copy, Default)]
pub struct MigrateDbTool;

impl SetupTool for MigrateDbTool {
    fn name(&self) -> &'static str {
        "migrate_db"
    }

    fn requires_confirm(&self) -> bool {
        false
    }

    fn execute(&self, ctx: &ToolContext, input: &ToolInput) -> crate::error::Result<ToolResult> {
        let dry_run = input.args.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
        let deployment = ctx
            .session
            .deployment
            .unwrap_or(DeploymentChoice::Micro);
        let spec = HostSpecChecker::collect();
        let plan = DeployPlanner::plan(deployment, InstallStrategy::Prebuilt, &spec, None)
            .map_err(|e| crate::error::SetupError::Internal(e.to_string()))?;
        let _plan: &DeployPlan = &plan;

        let runner = HostScriptRunner::new(ctx.repo_root.clone());
        let stack = StackRunner::new(runner);
        let out = stack.run(StackAction::Migrate, &plan, false, dry_run)?;

        Ok(ToolResult {
            ok: true,
            message: out.command,
            artifacts: vec![],
        })
    }
}
