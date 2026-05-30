//! `clawz setup deps|stack` — host bootstrap and Compose lifecycle.

use anyhow::{anyhow, Context, Result};
use clawz_setup::{
    DependencyInstaller, DeploymentChoice, HostExecPolicy, HostScriptRunner,
    HostSpecChecker, InstallStrategy, StackAction, StackRunner, SetupToolRegistry, ToolContext,
    ToolInput, ConfirmGate,
};

#[derive(Debug, Clone, Copy)]
pub struct SetupDepsOptions {
    pub dry_run: bool,
    pub with_web: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SetupStackOptions {
    pub build: bool,
    pub with_web: bool,
    pub dry_run: bool,
    pub deployment: Option<DeploymentChoice>,
}

pub fn run_deps(opts: SetupDepsOptions) -> Result<()> {
    ensure_host_exec(opts.dry_run)?;
    let runner = HostScriptRunner::from_env().context("resolve ClawZ repo root")?;
    let installer = DependencyInstaller::new(runner);
    let outputs = installer.install_all(opts.with_web, opts.dry_run)?;
    for out in &outputs {
        println!("{}", out.command);
        if !out.dry_run && !out.stderr.is_empty() {
            eprint!("{}", out.stderr);
        }
    }
    if opts.dry_run {
        println!("(dry-run — no commands executed)");
    } else {
        println!("Host dependencies ready.");
    }
    Ok(())
}

pub fn run_stack(opts: SetupStackOptions) -> Result<()> {
    ensure_host_exec(opts.dry_run)?;
    let runner = HostScriptRunner::from_env().context("resolve ClawZ repo root")?;
    let spec = HostSpecChecker::collect();
    let deployment = opts.deployment.unwrap_or(DeploymentChoice::Micro);
    let strategy = if opts.build {
        InstallStrategy::Build
    } else {
        InstallStrategy::Prebuilt
    };

    if matches!(strategy, InstallStrategy::Prebuilt | InstallStrategy::Build) {
        if !opts.dry_run {
            let mut reg = SetupToolRegistry::with_default_tools();
            reg.set_confirm_gate(ConfirmGate::new("yes-install"));
            let ctx = ToolContext {
                session: clawz_setup::SetupSession::new(clawz_setup::SetupPlatform::Linux),
                confirm_token: Some("yes-install".into()),
                repo_root: runner.repo_root().to_path_buf(),
                env_path: runner.repo_root().join(".env"),
            };
            let dep_out = reg.run(
                "install_deps",
                &ctx,
                &ToolInput {
                    args: serde_json::json!({ "dry_run": false, "with_web": opts.with_web }),
                },
            )
            .map_err(|e| anyhow!("{e}"))?;
            if !dep_out.ok {
                anyhow::bail!("install_deps failed: {}", dep_out.message);
            }
        } else {
            run_deps(SetupDepsOptions {
                dry_run: true,
                with_web: opts.with_web,
            })?;
        }
    }

    let plan = StackRunner::plan(deployment, strategy, &spec, None)
        .map_err(|e| anyhow!("{e}"))?;
    let stack = StackRunner::new(runner);
    let out = stack
        .run(StackAction::Up, &plan, opts.with_web, opts.dry_run)
        .map_err(|e| anyhow!("{e}"))?;

    println!("{}", out.command);
    if !opts.dry_run {
        println!("Stack started: {}", StackRunner::compose_argv_hint(&plan));
        println!("Run `clawz doctor` to verify.");
    } else {
        println!("(dry-run — no stack started)");
    }
    Ok(())
}

fn ensure_host_exec(dry_run: bool) -> Result<()> {
    if HostExecPolicy::allowed() || dry_run {
        return Ok(());
    }
    anyhow::bail!(
        "host bootstrap disabled in this environment (gateway container). Run on the host:\n  {}",
        HostExecPolicy::suggested_install_command()
    )
}
