//! SSH terminal backend via system `ssh` client.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use clawz_core::error::{ClawzError, Result};

use super::{shell_escape, truncate_output, ExecResult, TerminalBackend, MAX_EXEC_OUTPUT_BYTES};

pub struct SshBackend {
    workdir: PathBuf,
    host: String,
    user: String,
    identity_file: Option<PathBuf>,
    remote_workdir: String,
}

impl SshBackend {
    pub fn from_env(workdir: PathBuf) -> Result<Self> {
        let host = std::env::var("CLAWZ_SSH_HOST")
            .map_err(|_| ClawzError::Config("CLAWZ_SSH_HOST required for ssh backend".into()))?;
        let user = std::env::var("CLAWZ_SSH_USER").unwrap_or_else(|_| "root".into());
        let identity_file = std::env::var("CLAWZ_SSH_IDENTITY_FILE")
            .ok()
            .map(PathBuf::from);
        let remote_workdir = std::env::var("CLAWZ_SSH_WORKDIR")
            .unwrap_or_else(|_| "/tmp/clawz".into());
        Ok(Self {
            workdir,
            host,
            user,
            identity_file,
            remote_workdir,
        })
    }

    fn remote_path(&self, path: &Path) -> String {
        if path.starts_with(&self.workdir) {
            let rel = path
                .strip_prefix(&self.workdir)
                .unwrap_or(path)
                .to_string_lossy();
            format!(
                "{}/{}",
                self.remote_workdir.trim_end_matches('/'),
                rel.trim_start_matches('/')
            )
        } else {
            path.to_string_lossy().into_owned()
        }
    }

    async fn ssh_exec_raw(&self, remote_cmd: &str, timeout_secs: u64) -> Result<ExecResult> {
        let target = format!("{}@{}", self.user, self.host);
        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new");
        if let Some(key) = &self.identity_file {
            cmd.arg("-i").arg(key);
        }
        cmd.arg(&target).arg(remote_cmd);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| ClawzError::Tool(format!("ssh spawn failed: {e}")))?;

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| ClawzError::Tool(format!("ssh timed out after {timeout_secs}s")))?
        .map_err(|e| ClawzError::Tool(format!("ssh failed: {e}")))?;

        Ok(ExecResult {
            stdout: truncate_output(&output.stdout, MAX_EXEC_OUTPUT_BYTES),
            stderr: truncate_output(&output.stderr, MAX_EXEC_OUTPUT_BYTES),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

#[async_trait]
impl TerminalBackend for SshBackend {
    fn backend_id(&self) -> &str {
        "ssh"
    }

    fn workdir(&self) -> PathBuf {
        self.workdir.clone()
    }

    async fn exec(
        &self,
        command: &str,
        _cwd: Option<&Path>,
        timeout_secs: u64,
        _env: &HashMap<String, String>,
    ) -> Result<ExecResult> {
        let remote_cmd = format!(
            "cd {} && {}",
            shell_escape(&self.remote_workdir),
            command
        );
        self.ssh_exec_raw(&remote_cmd, timeout_secs).await
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let remote = shell_escape(&self.remote_path(path));
        let out = self
            .ssh_exec_raw(&format!("cat -- {remote}"), 60)
            .await?;
        if out.success() {
            Ok(out.stdout.into_bytes())
        } else {
            Err(ClawzError::Tool(out.stderr))
        }
    }

    async fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let remote = shell_escape(&self.remote_path(path));
        let encoded = B64.encode(content);
        let cmd = format!(
            "mkdir -p -- $(dirname {remote}) && echo {data} | base64 -d > {remote}",
            data = shell_escape(&encoded),
            remote = remote
        );
        let out = self.ssh_exec_raw(&cmd, 120).await?;
        if out.success() {
            Ok(())
        } else {
            Err(ClawzError::Tool(out.stderr))
        }
    }
}
