//! Host dependency installation via `scripts/setup-host-exec.sh`.

use crate::error::{Result, SetupError};
use crate::host_exec::{HostExecOutput, HostScriptRunner};

/// Component installable through `install-deps.sh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepComponent {
    Curl,
    Git,
    Docker,
    Rust,
    Node,
}

impl DepComponent {
    fn script_name(self) -> &'static str {
        match self {
            Self::Curl => "ensure_curl",
            Self::Git => "ensure_git",
            Self::Docker => "ensure_docker",
            Self::Rust => "ensure_rust",
            Self::Node => "ensure_node",
        }
    }
}

/// Wraps `install-deps.sh` helpers for wizard / CLI bootstrap.
#[derive(Debug, Clone)]
pub struct DependencyInstaller {
    runner: HostScriptRunner,
}

impl DependencyInstaller {
    pub fn new(runner: HostScriptRunner) -> Self {
        Self { runner }
    }

    pub fn from_env() -> Result<Self> {
        Ok(Self::new(HostScriptRunner::from_env()?))
    }

    pub fn runner(&self) -> &HostScriptRunner {
        &self.runner
    }

    /// Install selected components (default: curl, git, docker).
    pub fn install(
        &self,
        components: &[DepComponent],
        dry_run: bool,
    ) -> Result<Vec<HostExecOutput>> {
        let list = if components.is_empty() {
            vec![
                DepComponent::Curl,
                DepComponent::Git,
                DepComponent::Docker,
            ]
        } else {
            components.to_vec()
        };

        let mut outputs = Vec::with_capacity(list.len());
        for component in list {
            let out = self
                .runner
                .run_setup_host(component.script_name(), &[], dry_run)?;
            if !out.success() {
                return Err(SetupError::Internal(format!(
                    "{} failed (exit {:?}): {}",
                    component.script_name(),
                    out.exit_code,
                    trim_lines(&out.stderr, 8)
                )));
            }
            outputs.push(out);
        }
        Ok(outputs)
    }

    pub fn install_all(&self, with_web: bool, dry_run: bool) -> Result<Vec<HostExecOutput>> {
        let mut components = vec![
            DepComponent::Curl,
            DepComponent::Git,
            DepComponent::Docker,
        ];
        if with_web {
            components.push(DepComponent::Node);
        }
        self.install(&components, dry_run)
    }
}

fn trim_lines(s: &str, max: usize) -> String {
    s.lines().take(max).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dry_run_returns_preview_commands() {
        let installer = DependencyInstaller::from_env().expect("repo root");
        let outs = installer
            .install(&[DepComponent::Curl], true)
            .expect("dry-run");
        assert_eq!(outs.len(), 1);
        assert!(outs[0].dry_run);
        assert!(outs[0].command.contains("ensure_curl"));
    }
}
