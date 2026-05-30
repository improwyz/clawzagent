//! Allowlisted setup tools (no arbitrary shell).

pub mod init_workspace;
mod spec_check;
mod write_env;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use init_workspace::{InitWorkspaceTool, init_workspace_at, workspace_root};
pub use spec_check::SpecCheckTool;
pub use write_env::WriteEnvTool;

use crate::error::{Result, SetupError};
use crate::types::{SetupArtifact, SetupSession};

/// Context passed to every setup tool invocation.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub session: SetupSession,
    /// User-issued token for destructive tools (must match [`ConfirmGate`]).
    pub confirm_token: Option<String>,
    /// Repository root (for `.env` and compose files).
    pub repo_root: std::path::PathBuf,
    /// Path to the operator `.env` file (usually `repo_root/.env`).
    pub env_path: std::path::PathBuf,
}

/// JSON arguments for a tool call (shape depends on tool).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolInput {
    #[serde(default)]
    pub args: Value,
}

/// Outcome of a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<SetupArtifact>,
}

/// Allowlisted, structured setup operation (no shell).
pub trait SetupTool: Send + Sync {
    fn name(&self) -> &'static str;

    /// When true, [`SetupToolRegistry::run`] requires a matching confirm token.
    fn requires_confirm(&self) -> bool;

    fn execute(&self, ctx: &ToolContext, input: &ToolInput) -> Result<ToolResult>;
}

/// Enforces explicit user confirmation before destructive tools run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmGate {
    expected: String,
}

impl ConfirmGate {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            expected: token.into(),
        }
    }

    pub fn expected(&self) -> &str {
        &self.expected
    }

    pub fn verify(&self, provided: Option<&str>) -> Result<()> {
        match provided {
            Some(p) if p == self.expected => Ok(()),
            Some(_) => Err(SetupError::InvalidTransition(
                "confirm token mismatch".into(),
            )),
            None => Err(SetupError::InvalidTransition(
                "confirm token required for this tool".into(),
            )),
        }
    }
}

/// Registry of allowlisted tools by name.
#[derive(Default)]
pub struct SetupToolRegistry {
    tools: Vec<Box<dyn SetupTool>>,
    gate: Option<ConfirmGate>,
}

impl SetupToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_tools() -> Self {
        let mut reg = Self::new();
        reg.register(SpecCheckTool);
        reg.register(WriteEnvTool);
        reg.register(InitWorkspaceTool);
        reg
    }

    pub fn set_confirm_gate(&mut self, gate: ConfirmGate) {
        self.gate = Some(gate);
    }

    pub fn register<T: SetupTool + 'static>(&mut self, tool: T) {
        self.tools.push(Box::new(tool));
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    pub fn run(&self, name: &str, ctx: &ToolContext, input: &ToolInput) -> Result<ToolResult> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| SetupError::InvalidTransition(format!("unknown tool: {name}")))?;

        if tool.requires_confirm() {
            let gate = self.gate.as_ref().ok_or_else(|| {
                SetupError::InvalidTransition("confirm gate not configured".into())
            })?;
            gate.verify(ctx.confirm_token.as_deref())?;
        }

        tool.execute(ctx, input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SetupPlatform;

    struct EchoTool;

    impl SetupTool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn requires_confirm(&self) -> bool {
            true
        }

        fn execute(&self, _ctx: &ToolContext, input: &ToolInput) -> Result<ToolResult> {
            Ok(ToolResult {
                ok: true,
                message: input.args.to_string(),
                artifacts: vec![],
            })
        }
    }

    fn test_ctx(token: Option<&str>) -> ToolContext {
        ToolContext {
            session: SetupSession::new(SetupPlatform::Linux),
            confirm_token: token.map(str::to_string),
            repo_root: std::path::PathBuf::from("/tmp"),
            env_path: std::path::PathBuf::from("/tmp/.env"),
        }
    }

    #[test]
    fn registry_runs_tool_without_confirm_when_not_required() {
        let mut reg = SetupToolRegistry::new();
        reg.register(SpecCheckTool);
        let ctx = test_ctx(None);
        let out = reg.run("spec_check", &ctx, &ToolInput::default()).unwrap();
        assert!(out.ok, "spec_check should complete: {}", out.message);
        assert!(!out.message.is_empty());
    }

    #[test]
    fn confirm_gate_blocks_destructive_tool_without_token() {
        let mut reg = SetupToolRegistry::new();
        reg.set_confirm_gate(ConfirmGate::new("yes-write"));
        reg.register(EchoTool);
        let ctx = test_ctx(None);
        let err = reg.run("echo", &ctx, &ToolInput::default()).unwrap_err();
        assert!(matches!(err, SetupError::InvalidTransition(_)));
    }

    #[test]
    fn confirm_gate_allows_matching_token() {
        let mut reg = SetupToolRegistry::new();
        reg.set_confirm_gate(ConfirmGate::new("yes-write"));
        reg.register(EchoTool);
        let ctx = test_ctx(Some("yes-write"));
        let out = reg.run("echo", &ctx, &ToolInput::default()).unwrap();
        assert!(out.ok);
    }

    #[test]
    fn default_registry_includes_core_tools() {
        let reg = SetupToolRegistry::with_default_tools();
        let names = reg.names();
        assert!(names.contains(&"spec_check"));
        assert!(names.contains(&"write_env"));
        assert!(names.contains(&"init_workspace"));
    }
}
