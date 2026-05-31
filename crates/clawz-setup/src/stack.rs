//! Docker Compose stack lifecycle (prebuilt / build overlays).

use crate::deploy::{DeployPlan, DeployPlanner};
use crate::error::{Result, SetupError};
use crate::host_exec::{HostExecOutput, HostScriptRunner};
use crate::spec::HostSpecReport;
use crate::types::{DeploymentChoice, InstallStrategy};

/// Stack operations mapped to `scripts/setup-host-exec.sh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackAction {
    Up,
    Down,
    Migrate,
}

/// Runs compose up/down/migrate using the deploy planner for strategy selection.
#[derive(Debug, Clone)]
pub struct StackRunner {
    runner: HostScriptRunner,
}

impl StackRunner {
    pub fn new(runner: HostScriptRunner) -> Self {
        Self { runner }
    }

    pub fn from_env() -> Result<Self> {
        Ok(Self::new(HostScriptRunner::from_env()?))
    }

    pub fn runner(&self) -> &HostScriptRunner {
        &self.runner
    }

    pub fn plan(
        deployment: DeploymentChoice,
        strategy: InstallStrategy,
        spec: &HostSpecReport,
        image_tag: Option<&str>,
    ) -> Result<DeployPlan> {
        DeployPlanner::plan(deployment, strategy, spec, image_tag)
            .map_err(|e| SetupError::Internal(e.to_string()))
    }

    pub fn run(
        &self,
        action: StackAction,
        plan: &DeployPlan,
        with_web: bool,
        dry_run: bool,
    ) -> Result<HostExecOutput> {
        match action {
            StackAction::Up => self.stack_up(plan, with_web, dry_run),
            StackAction::Down => self.runner.run_setup_host("stack_down", &[], dry_run),
            StackAction::Migrate => self.runner.run_setup_host("migrate_db", &[], dry_run),
        }
    }

    fn stack_up(&self, plan: &DeployPlan, with_web: bool, dry_run: bool) -> Result<HostExecOutput> {
        if plan.standalone_source.is_some() {
            return Err(SetupError::Internal(
                "stack_up does not apply to standalone source installs".into(),
            ));
        }

        let build = matches!(plan.strategy, InstallStrategy::Build);
        let build_arg = if build { "1" } else { "0" };
        let web_arg = if with_web { "1" } else { "0" };

        let mut out = self
            .runner
            .run_setup_host("stack_up", &[build_arg, web_arg], dry_run)?;

        if !dry_run {
            for (key, value) in &plan.env {
                out.stdout.push_str(&format!("\n[env] {key}={value}"));
            }
        }

        if !out.success() {
            return Err(SetupError::Internal(format!(
                "stack_up failed (exit {:?}): {}",
                out.exit_code,
                out.stderr.lines().take(12).collect::<Vec<_>>().join("\n")
            )));
        }
        Ok(out)
    }

    /// Human-readable preview of compose file args (for wizard UI).
    pub fn compose_argv_hint(plan: &DeployPlan) -> String {
        let args = plan.compose_file_args();
        if args.is_empty() {
            return "docker compose".into();
        }
        format!("docker compose {}", args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::HostSpecReport;

    fn docker_spec(gh: bool) -> HostSpecReport {
        HostSpecReport {
            os: "Linux".into(),
            arch: "x86_64".into(),
            ram_mb: Some(8192),
            disk_free_gb: Some(40),
            cpu_count: Some(4),
            docker_available: true,
            docker_detail: None,
            compose_v2_available: true,
            compose_detail: None,
            rust_version: Some("1.87.0".into()),
            rust_meets_minimum: Some(true),
            node_version: None,
            github_token_present: gh,
            summary: String::new(),
            warnings: vec![],
        }
    }

    #[test]
    fn stack_up_dry_run_includes_build_flag() {
        let runner = HostScriptRunner::from_env().unwrap();
        let stack = StackRunner::new(runner);
        let spec = docker_spec(true);
        let plan = StackRunner::plan(DeploymentChoice::Micro, InstallStrategy::Build, &spec, None)
            .unwrap();
        let out = stack
            .run(StackAction::Up, &plan, false, true)
            .expect("dry-run");
        assert!(out.command.contains("stack_up"));
        assert!(out.command.contains("1"));
    }

    #[test]
    fn compose_argv_hint_prebuilt() {
        let spec = docker_spec(true);
        let plan = StackRunner::plan(
            DeploymentChoice::Micro,
            InstallStrategy::Prebuilt,
            &spec,
            None,
        )
        .unwrap();
        let hint = StackRunner::compose_argv_hint(&plan);
        assert!(hint.contains("docker-compose.prebuilt.yml"));
    }
}
