use crate::tools::tool_trait::{Tool, ToolContext};
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::{ToolResult, ToolSchema};
use serde_json::Value;
use std::process::Stdio;
use tokio::process::Command;

pub struct GitTool;

impl GitTool {
    pub fn new() -> Self {
        Self
    }

    async fn run_git(args: &[&str], working_dir: Option<&str>) -> Result<GitOutput, ClawzError> {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Prevent interactive prompts
            .env("GIT_TERMINAL_PROMPT", "0");

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            cmd.spawn()
                .map_err(|e| ClawzError::Tool(format!("failed to spawn git: {e}")))?
                .wait_with_output(),
        )
        .await
        .map_err(|_| ClawzError::Tool("git command timed out".into()))?
        .map_err(|e| ClawzError::Tool(format!("git failed: {e}")))?;

        Ok(GitOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

#[derive(Debug)]
struct GitOutput {
    stdout: String,
    stderr: String,
    success: bool,
    exit_code: i32,
}

impl GitOutput {
    fn to_json(&self) -> String {
        serde_json::json!({
            "stdout": self.stdout,
            "stderr": self.stderr,
            "exit_code": self.exit_code,
            "success": self.success
        })
        .to_string()
    }
}

impl Default for GitTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Git operations: status, diff, log, commit, branch, checkout, push, pull. Formats output for LLM consumption."
    }


    fn primitive(&self) -> ActionPrimitive { ActionPrimitive::Execute }
    fn risk(&self) -> RiskLevel { RiskLevel::High }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "git".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["status", "diff", "log", "commit", "branch",
                                 "checkout", "push", "pull", "add", "reset",
                                 "stash", "tag", "remote", "fetch", "merge"],
                        "description": "Git operation to perform"
                    },
                    "repo_path": {
                        "type": "string",
                        "description": "Path to git repository (default: current directory)"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Additional arguments for the git command"
                    },
                    "message": {
                        "type": "string",
                        "description": "Commit message (for commit operation)"
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch name (for branch/checkout operations)"
                    },
                    "remote": {
                        "type": "string",
                        "description": "Remote name (default: origin)"
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files to stage (for add operation)"
                    },
                    "max_commits": {
                        "type": "integer",
                        "description": "Max commits to show in log (default: 20)"
                    }
                },
                "required": ["operation"]
            }),
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: Value,
    ) -> Result<ToolResult, ClawzError> {
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("operation required".into()))?;

        let repo_path = args["repo_path"].as_str();

        // Extra args from caller
        let extra_args: Vec<String> = args["args"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        let output = match operation {
            "status" => {
                let mut git_args = vec!["status", "--porcelain=v2", "--branch"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                // Format nicely
                let formatted = format_git_status(&out.stdout);
                serde_json::json!({
                    "status": formatted,
                    "raw": out.stdout,
                    "success": out.success
                })
                .to_string()
            }

            "diff" => {
                let mut git_args = vec!["diff"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                serde_json::json!({
                    "diff": out.stdout,
                    "stderr": out.stderr,
                    "success": out.success
                })
                .to_string()
            }

            "log" => {
                let max_commits_str = format!("--max-count={}", args["max_commits"].as_u64().unwrap_or(20));
                let format_arg = "--pretty=format:%H|%an|%ae|%ai|%s";
                let mut git_args = vec![
                    "log",
                    max_commits_str.as_str(),
                    format_arg,
                ];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;

                let commits: Vec<Value> = out
                    .stdout
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|line| {
                        let parts: Vec<&str> = line.splitn(5, '|').collect();
                        serde_json::json!({
                            "hash": parts.first().unwrap_or(&""),
                            "author": parts.get(1).unwrap_or(&""),
                            "email": parts.get(2).unwrap_or(&""),
                            "date": parts.get(3).unwrap_or(&""),
                            "message": parts.get(4).unwrap_or(&"")
                        })
                    })
                    .collect();

                serde_json::json!({
                    "commits": commits,
                    "count": commits.len(),
                    "success": out.success
                })
                .to_string()
            }

            "commit" => {
                let message = args["message"]
                    .as_str()
                    .ok_or_else(|| ClawzError::Validation("message required for commit".into()))?;

                let mut git_args = vec!["commit", "-m", message];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "branch" => {
                if let Some(branch_name) = args["branch"].as_str() {
                    // Create or switch to branch
                    let mut git_args = vec!["branch", branch_name];
                    let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                    git_args.extend(extra_refs);
                    let out = Self::run_git(&git_args, repo_path).await?;
                    out.to_json()
                } else {
                    // List branches
                    let mut git_args = vec!["branch", "-a", "--format=%(refname:short)|%(HEAD)"];
                    let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                    git_args.extend(extra_refs);
                    let out = Self::run_git(&git_args, repo_path).await?;

                    let branches: Vec<Value> = out
                        .stdout
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(|line| {
                            let parts: Vec<&str> = line.splitn(2, '|').collect();
                            let name = parts.first().unwrap_or(&"").trim();
                            let is_current = parts.get(1).unwrap_or(&"").trim() == "*";
                            serde_json::json!({
                                "name": name,
                                "current": is_current
                            })
                        })
                        .collect();

                    serde_json::json!({
                        "branches": branches,
                        "success": out.success
                    })
                    .to_string()
                }
            }

            "checkout" => {
                let branch = args["branch"]
                    .as_str()
                    .ok_or_else(|| ClawzError::Validation("branch required for checkout".into()))?;

                let mut git_args = vec!["checkout", branch];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "push" => {
                let remote = args["remote"].as_str().unwrap_or("origin");
                let mut git_args = vec!["push", remote];
                if let Some(branch) = args["branch"].as_str() {
                    git_args.push(branch);
                }
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "pull" => {
                let remote = args["remote"].as_str().unwrap_or("origin");
                let mut git_args = vec!["pull", remote];
                if let Some(branch) = args["branch"].as_str() {
                    git_args.push(branch);
                }
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "add" => {
                let files: Vec<String> = if let Some(arr) = args["files"].as_array() {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                } else {
                    vec![".".to_string()]
                };

                let mut git_args = vec!["add".to_string()];
                git_args.extend(files);
                let refs: Vec<&str> = git_args.iter().map(|s| s.as_str()).collect();
                let out = Self::run_git(&refs, repo_path).await?;
                out.to_json()
            }

            "reset" => {
                let mut git_args = vec!["reset"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "stash" => {
                let mut git_args = vec!["stash"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "tag" => {
                let mut git_args = vec!["tag"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "remote" => {
                let mut git_args = vec!["remote", "-v"];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "fetch" => {
                let remote = args["remote"].as_str().unwrap_or("origin");
                let mut git_args = vec!["fetch", remote];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            "merge" => {
                let branch = args["branch"]
                    .as_str()
                    .ok_or_else(|| ClawzError::Validation("branch required for merge".into()))?;
                let mut git_args = vec!["merge", branch];
                let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                git_args.extend(extra_refs);
                let out = Self::run_git(&git_args, repo_path).await?;
                out.to_json()
            }

            other => {
                return Err(ClawzError::Validation(format!("unknown operation: {}", other)));
            }
        };

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error: false,
        })
    }
}

fn format_git_status(porcelain_v2: &str) -> String {
    let mut lines = Vec::new();
    for line in porcelain_v2.lines() {
        if line.starts_with("# branch.head ") {
            lines.push(format!(
                "Branch: {}",
                line.strip_prefix("# branch.head ").unwrap_or(line)
            ));
        } else if line.starts_with("# branch.upstream ") {
            lines.push(format!(
                "Upstream: {}",
                line.strip_prefix("# branch.upstream ").unwrap_or(line)
            ));
        } else if line.starts_with("# branch.ab ") {
            lines.push(format!(
                "Ahead/Behind: {}",
                line.strip_prefix("# branch.ab ").unwrap_or(line)
            ));
        } else if let Some(rest) = line.strip_prefix("1 ") {
            // Modified/added tracked file
            let parts: Vec<&str> = rest.splitn(9, ' ').collect();
            if let (Some(xy), Some(path)) = (parts.first(), parts.get(8)) {
                lines.push(format!("  {} {}", xy, path));
            }
        } else if let Some(rest) = line.strip_prefix("2 ") {
            // Renamed file
            let parts: Vec<&str> = rest.splitn(10, ' ').collect();
            if let (Some(xy), Some(path)) = (parts.first(), parts.get(9)) {
                lines.push(format!("  R {} {}", xy, path));
            }
        } else if let Some(rest) = line.strip_prefix("? ") {
            lines.push(format!("  ?? {}", rest));
        }
    }
    if lines.len() <= 1 {
        lines.push("  nothing to commit, working tree clean".to_string());
    }
    lines.join("\n")
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
    fn test_git_name() {
        assert_eq!(GitTool::new().name(), "git");
    }

    #[test]
    fn test_git_schema() {
        let schema = GitTool::new().schema();
        assert_eq!(schema.name, "git");
        let ops = &schema.parameters["properties"]["operation"]["enum"];
        assert!(ops.as_array().unwrap().contains(&serde_json::json!("status")));
        assert!(ops.as_array().unwrap().contains(&serde_json::json!("commit")));
    }

    #[tokio::test]
    async fn test_git_status_in_repo() {
        // This test uses the actual git repo in the workspace
        let tool = GitTool::new();
        let ctx = make_ctx();
        let result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "status",
                    "repo_path": "/home/ubuntu/clawzagent"
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert!(parsed["success"].as_bool().unwrap_or(false));
    }

    #[tokio::test]
    async fn test_git_log_in_repo() {
        let tool = GitTool::new();
        let ctx = make_ctx();
        let result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "log",
                    "repo_path": "/home/ubuntu/clawzagent",
                    "max_commits": 3
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert!(parsed["commits"].as_array().is_some());
    }

    #[tokio::test]
    async fn test_git_missing_operation() {
        let tool = GitTool::new();
        let ctx = make_ctx();
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }
}
