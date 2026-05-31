//! Factory for the configured [`TerminalBackend`].

use std::path::PathBuf;
use std::sync::Arc;

use clawz_core::error::Result;

use super::TerminalBackend;
use super::config::{TerminalBackendKind, resolve_backend_kind};
use super::docker::DockerBackend;
use super::local::LocalBackend;
use super::ssh::SshBackend;

/// Default workspace for terminal sandboxes.
pub fn default_workdir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAWZ_TERMINAL_WORKDIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("CLAWZ_SANDBOX_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("CLAWZ_HOME") {
        return PathBuf::from(home).join("workspace");
    }
    if let Ok(ws) = std::env::var("CLAWZ_WORKSPACE") {
        return PathBuf::from(ws);
    }
    PathBuf::from("/tmp/clawz")
}

/// Create the terminal backend for the current environment.
pub fn create_terminal_backend() -> Result<Arc<dyn TerminalBackend>> {
    let workdir = default_workdir();
    let _ = std::fs::create_dir_all(&workdir);

    let backend: Arc<dyn TerminalBackend> = match resolve_backend_kind() {
        TerminalBackendKind::Local => Arc::new(LocalBackend::new(workdir)),
        TerminalBackendKind::Docker => Arc::new(DockerBackend::new(workdir)?),
        TerminalBackendKind::Ssh => Arc::new(SshBackend::from_env(workdir)?),
    };

    tracing::info!(
        backend = backend.backend_id(),
        workdir = %backend.workdir().display(),
        "terminal backend ready"
    );

    Ok(backend)
}
