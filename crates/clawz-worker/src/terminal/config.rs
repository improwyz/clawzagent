//! Terminal backend selection from environment and deployment mode.

use clawz_core::deployment::DeploymentMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalBackendKind {
    Local,
    Docker,
    Ssh,
}

impl TerminalBackendKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "local" => Some(Self::Local),
            "docker" => Some(Self::Docker),
            "ssh" => Some(Self::Ssh),
            _ => None,
        }
    }
}

/// Resolve backend: `CLAWZ_TERMINAL_BACKEND` overrides deployment default.
pub fn resolve_backend_kind() -> TerminalBackendKind {
    if let Ok(raw) = std::env::var("CLAWZ_TERMINAL_BACKEND") {
        if let Some(kind) = TerminalBackendKind::parse(&raw) {
            return kind;
        }
        tracing::warn!("unknown CLAWZ_TERMINAL_BACKEND={raw}, using deployment default");
    }

    match DeploymentMode::from_env() {
        DeploymentMode::Standalone => TerminalBackendKind::Local,
        DeploymentMode::Micro | DeploymentMode::Elastic => TerminalBackendKind::Docker,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kinds() {
        assert_eq!(TerminalBackendKind::parse("docker"), Some(TerminalBackendKind::Docker));
        assert_eq!(TerminalBackendKind::parse("LOCAL"), Some(TerminalBackendKind::Local));
        assert_eq!(TerminalBackendKind::parse("invalid"), None);
    }
}
