//! Pluggable terminal backends for shell and file tools (local, Docker, SSH).

mod config;
mod docker;
mod factory;
mod local;
mod ssh;

pub use config::{resolve_backend_kind, TerminalBackendKind};
pub use factory::{create_terminal_backend, default_workdir};
pub use local::LocalBackend;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use clawz_core::error::Result;

/// Result of a shell command executed in a terminal backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl ExecResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Sandbox for agent shell and file operations.
#[async_trait]
pub trait TerminalBackend: Send + Sync {
    fn backend_id(&self) -> &str;

    /// Host workspace directory (bind-mounted for Docker).
    fn workdir(&self) -> PathBuf;

    async fn exec(
        &self,
        command: &str,
        cwd: Option<&Path>,
        timeout_secs: u64,
        env: &HashMap<String, String>,
    ) -> Result<ExecResult>;

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>>;

    async fn write_file(&self, path: &Path, content: &[u8]) -> Result<()>;

    async fn cwd(&self) -> Result<PathBuf> {
        Ok(self.workdir())
    }

    /// List directory entries as JSON-compatible lines (name, type, size).
    async fn list_directory(&self, path: &Path) -> Result<String> {
        let cmd = format!(
            "ls -la -- {}",
            shell_escape(path.to_string_lossy().as_ref())
        );
        let out = self.exec(&cmd, Some(path.parent().unwrap_or(path)), 30, &HashMap::new()).await?;
        if out.success() {
            Ok(out.stdout)
        } else {
            Err(clawz_core::error::ClawzError::Tool(format!(
                "list_directory failed: {}",
                out.stderr
            )))
        }
    }

    async fn create_directory(&self, path: &Path, recursive: bool) -> Result<()> {
        let flag = if recursive { "-p" } else { "" };
        let cmd = format!(
            "mkdir {flag} -- {}",
            shell_escape(path.to_string_lossy().as_ref())
        );
        let out = self.exec(&cmd, None, 30, &HashMap::new()).await?;
        if out.success() {
            Ok(())
        } else {
            Err(clawz_core::error::ClawzError::Tool(out.stderr))
        }
    }

    async fn delete_path(&self, path: &Path) -> Result<()> {
        let cmd = format!(
            "rm -rf -- {}",
            shell_escape(path.to_string_lossy().as_ref())
        );
        let out = self.exec(&cmd, None, 30, &HashMap::new()).await?;
        if out.success() {
            Ok(())
        } else {
            Err(clawz_core::error::ClawzError::Tool(out.stderr))
        }
    }
}

pub(crate) fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub(crate) fn truncate_output(bytes: &[u8], max: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() > max {
        format!("{}...[truncated, {} bytes total]", &s[..max], s.len())
    } else {
        s.into_owned()
    }
}

pub const MAX_EXEC_OUTPUT_BYTES: usize = 1024 * 1024;

#[cfg(test)]
mod tests;
