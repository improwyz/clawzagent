//! Local process terminal backend (default for standalone).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use clawz_core::error::{ClawzError, Result};

use super::{truncate_output, ExecResult, TerminalBackend, MAX_EXEC_OUTPUT_BYTES};

pub struct LocalBackend {
    workdir: PathBuf,
}

impl LocalBackend {
    pub fn new(workdir: PathBuf) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl TerminalBackend for LocalBackend {
    fn backend_id(&self) -> &str {
        "local"
    }

    fn workdir(&self) -> PathBuf {
        self.workdir.clone()
    }

    async fn exec(
        &self,
        command: &str,
        cwd: Option<&Path>,
        timeout_secs: u64,
        env: &HashMap<String, String>,
    ) -> Result<ExecResult> {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        } else {
            cmd.current_dir(&self.workdir);
        }

        for (k, v) in env {
            cmd.env(k, v);
        }

        let child = cmd
            .spawn()
            .map_err(|e| ClawzError::Tool(format!("failed to spawn command: {e}")))?;

        let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
            .map_err(|_| ClawzError::Tool(format!("command timed out after {timeout_secs}s")))?
            .map_err(|e| ClawzError::Tool(format!("command execution failed: {e}")))?;

        Ok(ExecResult {
            stdout: truncate_output(&output.stdout, MAX_EXEC_OUTPUT_BYTES),
            stderr: truncate_output(&output.stderr, MAX_EXEC_OUTPUT_BYTES),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        tokio::fs::read(path)
            .await
            .map_err(|e| ClawzError::Tool(format!("read failed: {e}")))
    }

    async fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ClawzError::Tool(format!("mkdir failed: {e}")))?;
        }
        tokio::fs::write(path, content)
            .await
            .map_err(|e| ClawzError::Tool(format!("write failed: {e}")))
    }
}
