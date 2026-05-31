use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::terminal::{LocalBackend, TerminalBackend, create_terminal_backend, default_workdir};
use crate::tools::tool_trait::{Tool, ToolContext};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use clawz_core::types::{ToolResult, ToolSchema};
use serde_json::Value;

/// Default allowed commands. Operators can override via env var CLAWZ_SHELL_ALLOW.
const DEFAULT_ALLOWED: &[&str] = &[
    "echo",
    "cat",
    "ls",
    "pwd",
    "date",
    "env",
    "printenv",
    "grep",
    "find",
    "wc",
    "sort",
    "uniq",
    "head",
    "tail",
    "sed",
    "awk",
    "cut",
    "tr",
    "jq",
    "curl",
    "wget",
    "python3",
    "python",
    "node",
    "ruby",
    "go",
    "cargo",
    "npm",
    "yarn",
    "pip",
    "pip3",
    "make",
    "cmake",
    "gcc",
    "g++",
    "rustc",
    "git",
    "diff",
    "patch",
    "tar",
    "zip",
    "unzip",
    "gzip",
    "gunzip",
    "mkdir",
    "cp",
    "mv",
    "rm",
    "touch",
    "chmod",
    "chown",
    "ps",
    "top",
    "kill",
    "sleep",
    "openssl",
    "ssh",
    "scp",
    "rsync",
    "docker",
    "kubectl",
    "helm",
    "psql",
    "mysql",
    "redis-cli",
    "mongo",
];

const BLOCKED_COMMANDS: &[&str] = &[
    "sudo",
    "su",
    "passwd",
    "useradd",
    "userdel",
    "groupadd",
    "visudo",
    "chroot",
    "mount",
    "umount",
    "fdisk",
    "mkfs",
    "dd",
    "format",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "iptables",
    "ufw",
    "firewall-cmd",
    "nc",
    "netcat",
    "ncat",
    "crontab",
];

const DEFAULT_TIMEOUT_SECS: u64 = 30;

pub struct ShellTool {
    allowed_commands: Vec<String>,
    backend: Arc<dyn TerminalBackend>,
}

impl ShellTool {
    pub fn new() -> Self {
        Self::with_backend(fallback_backend())
    }

    pub fn with_backend(backend: Arc<dyn TerminalBackend>) -> Self {
        let allowed = if let Ok(env_allow) = std::env::var("CLAWZ_SHELL_ALLOW") {
            env_allow.split(',').map(|s| s.trim().to_string()).collect()
        } else {
            DEFAULT_ALLOWED.iter().map(|s| s.to_string()).collect()
        };
        Self {
            allowed_commands: allowed,
            backend,
        }
    }

    pub fn with_allowed_commands(cmds: Vec<String>) -> Self {
        Self {
            allowed_commands: cmds,
            backend: fallback_backend(),
        }
    }

    fn extract_command_name(cmd: &str) -> &str {
        let trimmed = cmd.trim();
        let first_word = trimmed.split_whitespace().next().unwrap_or(trimmed);
        if let Some(base) = first_word.rsplit('/').next() {
            base
        } else {
            first_word
        }
    }

    fn is_allowed(&self, cmd: &str) -> Result<(), ClawzError> {
        let base = Self::extract_command_name(cmd);
        if BLOCKED_COMMANDS.contains(&base) {
            return Err(ClawzError::Validation(format!(
                "command '{base}' is blocked for security reasons"
            )));
        }
        if !self.allowed_commands.iter().any(|a| a == base) {
            return Err(ClawzError::Validation(format!(
                "command '{base}' is not in the allowed list"
            )));
        }
        Ok(())
    }
}

fn fallback_backend() -> Arc<dyn TerminalBackend> {
    create_terminal_backend().unwrap_or_else(|e| {
        tracing::warn!("terminal backend init failed ({e}), using local");
        Arc::new(LocalBackend::new(default_workdir()))
    })
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute shell commands via the configured terminal backend (local, docker, or ssh). Returns stdout, stderr, and exit code."
    }

    fn primitive(&self) -> ActionPrimitive {
        ActionPrimitive::Execute
    }
    fn risk(&self) -> RiskLevel {
        RiskLevel::High
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "shell".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for the command"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30, max: 300)"
                    },
                    "env": {
                        "type": "object",
                        "description": "Additional environment variables",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult, ClawzError> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("command required".into()))?;

        self.is_allowed(command)?;

        let timeout_secs = args["timeout_secs"]
            .as_u64()
            .unwrap_or(ctx.config.timeout_secs.min(DEFAULT_TIMEOUT_SECS))
            .min(300);

        let default_workdir = self.backend.workdir();
        let cwd = args["working_dir"]
            .as_str()
            .map(Path::new)
            .unwrap_or(default_workdir.as_path());

        let mut env = HashMap::new();
        if let Some(env_obj) = args["env"].as_object() {
            for (k, v) in env_obj {
                if let Some(val) = v.as_str() {
                    env.insert(k.clone(), val.to_string());
                }
            }
        }

        let output = self
            .backend
            .exec(command, Some(cwd), timeout_secs, &env)
            .await?;

        let result_json = serde_json::json!({
            "stdout": output.stdout,
            "stderr": output.stderr,
            "exit_code": output.exit_code,
            "success": output.success(),
            "backend": self.backend.backend_id(),
        })
        .to_string();

        Ok(ToolResult {
            tool_call_id: String::new(),
            output: result_json,
            is_error: !output.success(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolConfig;
    #[allow(unused_imports)]
    use serde_json::Value;

    fn make_ctx() -> ToolContext {
        ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: ToolConfig::default(),
        }
    }

    #[test]
    fn test_shell_name() {
        assert_eq!(ShellTool::new().name(), "shell");
    }

    #[test]
    fn test_blocked_command() {
        let tool = ShellTool::new();
        assert!(tool.is_allowed("sudo rm -rf /").is_err());
    }

    #[test]
    fn test_allowed_echo() {
        let tool = ShellTool::new();
        assert!(tool.is_allowed("echo hello").is_ok());
    }
}
