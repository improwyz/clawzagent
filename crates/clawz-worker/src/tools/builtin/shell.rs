use crate::tools::tool_trait::{Tool, ToolContext};
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::{ToolResult, ToolSchema};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Default allowed commands. Operators can override via env var CLAWZ_SHELL_ALLOW.
const DEFAULT_ALLOWED: &[&str] = &[
    "echo", "cat", "ls", "pwd", "date", "env", "printenv",
    "grep", "find", "wc", "sort", "uniq", "head", "tail",
    "sed", "awk", "cut", "tr", "jq", "curl", "wget",
    "python3", "python", "node", "ruby", "go",
    "cargo", "npm", "yarn", "pip", "pip3",
    "make", "cmake", "gcc", "g++", "rustc",
    "git", "diff", "patch",
    "tar", "zip", "unzip", "gzip", "gunzip",
    "mkdir", "cp", "mv", "rm", "touch", "chmod", "chown",
    "ps", "top", "kill", "sleep",
    "openssl", "ssh", "scp", "rsync",
    "docker", "kubectl", "helm",
    "psql", "mysql", "redis-cli", "mongo",
];

/// Commands that are always blocked.
const BLOCKED_COMMANDS: &[&str] = &[
    "sudo", "su", "passwd", "useradd", "userdel", "groupadd",
    "visudo", "chroot", "mount", "umount", "fdisk", "mkfs",
    "dd", "format", "shutdown", "reboot", "halt", "poweroff",
    "iptables", "ufw", "firewall-cmd",
    "nc", "netcat", "ncat",
    "crontab",
];

const MAX_OUTPUT_BYTES: usize = 1024 * 1024; // 1 MB
const DEFAULT_TIMEOUT_SECS: u64 = 30;

pub struct ShellTool {
    allowed_commands: Vec<String>,
}

impl ShellTool {
    pub fn new() -> Self {
        let allowed = if let Ok(env_allow) = std::env::var("CLAWZ_SHELL_ALLOW") {
            env_allow.split(',').map(|s| s.trim().to_string()).collect()
        } else {
            DEFAULT_ALLOWED.iter().map(|s| s.to_string()).collect()
        };

        Self {
            allowed_commands: allowed,
        }
    }

    pub fn with_allowed_commands(cmds: Vec<String>) -> Self {
        Self {
            allowed_commands: cmds,
        }
    }

    fn extract_command_name(cmd: &str) -> &str {
        // Handle paths like /usr/bin/echo -> "echo"
        let trimmed = cmd.trim();
        let first_word = trimmed.split_whitespace().next().unwrap_or(trimmed);
        // Strip path prefix
        if let Some(base) = first_word.rsplit('/').next() {
            base
        } else {
            first_word
        }
    }

    fn is_allowed(&self, cmd: &str) -> Result<(), ClawzError> {
        let base = Self::extract_command_name(cmd);

        // Always block certain commands
        if BLOCKED_COMMANDS.contains(&base) {
            return Err(ClawzError::Validation(format!(
                "command '{}' is blocked for security reasons",
                base
            )));
        }

        // Check allowlist
        if !self.allowed_commands.iter().any(|a| a == base) {
            return Err(ClawzError::Validation(format!(
                "command '{}' is not in the allowed list. Allowed: {:?}",
                base,
                &self.allowed_commands[..self.allowed_commands.len().min(10)]
            )));
        }

        Ok(())
    }
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
        "Execute shell commands with configurable timeout and sandboxing. Returns stdout, stderr, and exit code."
    }


    fn primitive(&self) -> ActionPrimitive { ActionPrimitive::Execute }
    fn risk(&self) -> RiskLevel { RiskLevel::High }
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

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: Value,
    ) -> Result<ToolResult, ClawzError> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("command required".into()))?;

        // Security check
        self.is_allowed(command)?;

        let timeout_secs = args["timeout_secs"]
            .as_u64()
            .unwrap_or(ctx.config.timeout_secs.min(DEFAULT_TIMEOUT_SECS))
            .min(300); // cap at 5 minutes

        // Build the command — run via sh -c for full shell features
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Working directory
        if let Some(dir) = args["working_dir"].as_str() {
            cmd.current_dir(dir);
        }

        // Extra env vars
        if let Some(env_obj) = args["env"].as_object() {
            for (k, v) in env_obj {
                if let Some(val) = v.as_str() {
                    cmd.env(k, val);
                }
            }
        }

        let child = cmd
            .spawn()
            .map_err(|e| ClawzError::Tool(format!("failed to spawn command: {e}")))?;

        // Wait with timeout
        let output = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| ClawzError::Tool(format!("command timed out after {}s", timeout_secs)))?
        .map_err(|e| ClawzError::Tool(format!("command execution failed: {e}")))?;

        let stdout = truncate_output(&output.stdout);
        let stderr = truncate_output(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

        let result_json = serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
            "success": exit_code == 0
        })
        .to_string();

        Ok(ToolResult {
            tool_call_id: String::new(),
            output: result_json,
            is_error: exit_code != 0,
        })
    }
}

fn truncate_output(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() > MAX_OUTPUT_BYTES {
        format!(
            "{}...[truncated, {} bytes total]",
            &s[..MAX_OUTPUT_BYTES],
            s.len()
        )
    } else {
        s.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolConfig;

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
    fn test_shell_schema() {
        let schema = ShellTool::new().schema();
        assert_eq!(schema.name, "shell");
        assert!(schema.parameters["properties"]["command"].is_object());
    }

    #[test]
    fn test_blocked_command() {
        let tool = ShellTool::new();
        let result = tool.is_allowed("sudo rm -rf /");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[test]
    fn test_allowed_command() {
        let tool = ShellTool::new();
        assert!(tool.is_allowed("echo hello").is_ok());
        assert!(tool.is_allowed("ls -la").is_ok());
    }

    #[test]
    fn test_not_allowed_command() {
        let tool = ShellTool::new();
        let result = tool.is_allowed("notacommand foo");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_echo_command() {
        let tool = ShellTool::new();
        let ctx = make_ctx();
        let result = tool
            .execute(
                &ctx,
                serde_json::json!({ "command": "echo hello_world" }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert!(parsed["stdout"].as_str().unwrap().contains("hello_world"));
        assert_eq!(parsed["exit_code"], 0);
    }

    #[tokio::test]
    async fn test_missing_command() {
        let tool = ShellTool::new();
        let ctx = make_ctx();
        let result = tool
            .execute(&ctx, serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_command_name() {
        assert_eq!(ShellTool::extract_command_name("echo hello"), "echo");
        assert_eq!(ShellTool::extract_command_name("/usr/bin/python3 script.py"), "python3");
        assert_eq!(ShellTool::extract_command_name("  ls -la  "), "ls");
    }
}
