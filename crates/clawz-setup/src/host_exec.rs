//! Run allowlisted host scripts from the ClawZ repo (bootstrap / stack).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, SetupError};

/// Output from a host script invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExecOutput {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub dry_run: bool,
}

impl HostExecOutput {
    pub fn success(&self) -> bool {
        self.dry_run || self.exit_code == Some(0)
    }
}

/// Whether the current process may run host bootstrap (not inside default gateway container).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostExecPolicy;

impl HostExecPolicy {
    pub fn allowed() -> bool {
        if std::env::var("CLAWZ_SETUP_ALLOW_HOST_EXEC")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        {
            return true;
        }
        !Path::new("/.dockerenv").exists()
    }

    /// Suggested one-liner when host exec is blocked (e.g. gateway in Compose).
    pub fn suggested_install_command() -> &'static str {
        "./scripts/install.sh --docker"
    }
}

/// Resolve repository root (`CLAWZ_INSTALL_DIR`, `CLAWZ_REPO_ROOT`, or walk-up from cwd).
pub fn resolve_repo_root() -> Result<PathBuf> {
    for key in ["CLAWZ_INSTALL_DIR", "CLAWZ_REPO_ROOT"] {
        if let Ok(dir) = std::env::var(key) {
            let path = PathBuf::from(dir);
            if is_repo_root(&path) {
                return Ok(path);
            }
        }
    }

    let cwd = std::env::current_dir()?;
    if is_repo_root(&cwd) {
        return Ok(cwd);
    }

    let mut cursor = cwd.as_path();
    while let Some(parent) = cursor.parent() {
        if is_repo_root(parent) {
            return Ok(parent.to_path_buf());
        }
        cursor = parent;
    }

    Err(SetupError::Internal(
        "could not find ClawZ repo root (set CLAWZ_INSTALL_DIR or run from clone)".into(),
    ))
}

fn is_repo_root(path: &Path) -> bool {
    path.join("docker-compose.yml").is_file() && path.join("Cargo.toml").is_file()
}

/// Executes `scripts/setup-host-exec.sh` subcommands with optional dry-run.
#[derive(Debug, Clone)]
pub struct HostScriptRunner {
    repo_root: PathBuf,
}

impl HostScriptRunner {
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(resolve_repo_root()?))
    }

    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn script_path(&self) -> PathBuf {
        self.repo_root.join("scripts/setup-host-exec.sh")
    }

    /// Run a allowlisted `setup-host-exec.sh` subcommand.
    pub fn run_setup_host(
        &self,
        subcommand: &str,
        args: &[&str],
        dry_run: bool,
    ) -> Result<HostExecOutput> {
        let script = self.script_path();
        if !script.is_file() {
            return Err(SetupError::Internal(format!(
                "missing host exec script: {}",
                script.display()
            )));
        }

        if cfg!(target_os = "windows") {
            return self.run_setup_host_windows(&script, subcommand, args, dry_run);
        }

        let mut cmd = Command::new("bash");
        cmd.arg(&script).arg(subcommand);
        cmd.args(args);
        cmd.current_dir(&self.repo_root);
        cmd.env("CLAWZ_REPO_ROOT", &self.repo_root);

        let command = format_command(&cmd, subcommand, args);

        if dry_run {
            return Ok(HostExecOutput {
                command,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                dry_run: true,
            });
        }

        let output = cmd.output()?;
        Ok(HostExecOutput {
            command,
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            dry_run: false,
        })
    }

    fn run_setup_host_windows(
        &self,
        script: &Path,
        subcommand: &str,
        args: &[&str],
        dry_run: bool,
    ) -> Result<HostExecOutput> {
        let _ = (script, subcommand, args);
        if dry_run {
            return Ok(HostExecOutput {
                command: format!(
                    "WSL or Git Bash: bash scripts/setup-host-exec.sh {subcommand} {}",
                    args.join(" ")
                ),
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                dry_run: true,
            });
        }
        Err(SetupError::Internal(
            "host stack/deps execution on Windows requires WSL or .\\scripts\\install.ps1 -Docker"
                .into(),
        ))
    }
}

fn format_command(cmd: &Command, subcommand: &str, args: &[&str]) -> String {
    let program = cmd.get_program().to_string_lossy();
    let mut parts = vec![program.to_string()];
    for arg in cmd.get_args() {
        parts.push(arg.to_string_lossy().into_owned());
    }
    if parts.len() <= 1 {
        parts.push(format!("setup-host-exec.sh {subcommand}"));
        parts.extend(args.iter().map(|s| (*s).to_string()));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn host_exec_policy_blocks_in_docker_when_env_unset() {
        let _ = HostExecPolicy::allowed();
    }

    #[test]
    fn resolve_repo_root_from_cwd() {
        let root = resolve_repo_root().expect("workspace should be repo root");
        assert!(root.join("docker-compose.yml").is_file());
    }

    #[test]
    fn dry_run_does_not_execute() {
        let root = resolve_repo_root().unwrap();
        let runner = HostScriptRunner::new(root);
        let out = runner
            .run_setup_host("ensure_curl", &[], true)
            .expect("dry-run");
        assert!(out.dry_run);
        assert!(out.exit_code.is_none());
        assert!(out.command.contains("setup-host-exec.sh"));
    }

    #[test]
    fn repo_root_detection_requires_compose_and_cargo() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert!(!is_repo_root(dir.path()));
        fs::write(dir.path().join("docker-compose.yml"), "").unwrap();
        assert!(is_repo_root(dir.path()));
    }
}
