use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::terminal::{LocalBackend, TerminalBackend, create_terminal_backend, default_workdir};
use crate::tools::tool_trait::{Tool, ToolContext};
use clawz_core::error::ClawzError;
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use clawz_core::types::{ToolResult, ToolSchema};

const MAX_READ_BYTES: usize = 5 * 1024 * 1024; // 5 MB
const MAX_WRITE_BYTES: usize = 10 * 1024 * 1024; // 10 MB

pub struct FileOpsTool {
    /// Directories the tool is allowed to access. Empty = terminal backend workdir.
    allowed_dirs: Vec<PathBuf>,
    backend: Arc<dyn TerminalBackend>,
}

impl FileOpsTool {
    pub fn new() -> Self {
        Self::with_backend(fallback_file_backend())
    }

    pub fn with_backend(backend: Arc<dyn TerminalBackend>) -> Self {
        Self {
            allowed_dirs: Vec::new(),
            backend,
        }
    }

    pub fn with_allowed_dirs(dirs: Vec<PathBuf>) -> Self {
        Self {
            allowed_dirs: dirs,
            backend: fallback_file_backend(),
        }
    }

    fn resolve_sandbox_dirs(&self) -> Vec<PathBuf> {
        if !self.allowed_dirs.is_empty() {
            return self.allowed_dirs.clone();
        }
        vec![self.backend.workdir()]
    }

    fn uses_local_fs(&self) -> bool {
        self.backend.backend_id() == "local"
    }

    fn validate_path(&self, path: &Path) -> Result<PathBuf, ClawzError> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let allowed = self.resolve_sandbox_dirs();

        for allowed_dir in &allowed {
            // Ensure the allowed_dir exists (create if /tmp/clawz)
            let allowed_canonical = allowed_dir
                .canonicalize()
                .unwrap_or_else(|_| allowed_dir.clone());

            if canonical.starts_with(&allowed_canonical) {
                return Ok(canonical);
            }
        }

        // For new files, check if parent is in allowed
        if let Some(parent) = path.parent() {
            let parent_canonical = parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf());
            for allowed_dir in &allowed {
                let allowed_canonical = allowed_dir
                    .canonicalize()
                    .unwrap_or_else(|_| allowed_dir.clone());
                if parent_canonical.starts_with(&allowed_canonical) {
                    return Ok(path.to_path_buf());
                }
            }
        }

        Err(ClawzError::Validation(format!(
            "path '{}' is outside sandbox directories: {:?}",
            path.display(),
            allowed
        )))
    }

    fn ensure_sandbox_exists(&self) -> Result<(), ClawzError> {
        for dir in self.resolve_sandbox_dirs() {
            if !dir.exists() {
                std::fs::create_dir_all(&dir).map_err(|e| {
                    ClawzError::Tool(format!(
                        "failed to create sandbox dir '{}': {e}",
                        dir.display()
                    ))
                })?;
            }
        }
        Ok(())
    }
}

fn fallback_file_backend() -> Arc<dyn TerminalBackend> {
    create_terminal_backend().unwrap_or_else(|e| {
        tracing::warn!("terminal backend init failed ({e}), using local");
        Arc::new(LocalBackend::new(default_workdir()))
    })
}

impl Default for FileOpsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for FileOpsTool {
    fn name(&self) -> &str {
        "file_ops"
    }

    fn description(&self) -> &str {
        "File system operations via the configured terminal backend (local, docker, or ssh). Sandboxed to allowed directories."
    }

    fn primitive(&self) -> ActionPrimitive {
        ActionPrimitive::Write
    }
    fn risk(&self) -> RiskLevel {
        RiskLevel::High
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "file_ops".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["read_file", "write_file", "list_directory",
                                 "create_directory", "delete_file", "file_info"],
                        "description": "File operation to perform"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory path"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write (for write_file)"
                    },
                    "encoding": {
                        "type": "string",
                        "enum": ["utf8", "base64"],
                        "description": "Content encoding (default: utf8)"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Create parent directories recursively (for create_directory)"
                    }
                },
                "required": ["operation", "path"]
            }),
        }
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult, ClawzError> {
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("operation required".into()))?;

        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("path required".into()))?;

        self.ensure_sandbox_exists()?;

        let path = PathBuf::from(path_str);
        let safe_path = self.validate_path(&path)?;

        let output = match operation {
            "read_file" => {
                let metadata = if self.uses_local_fs() {
                    std::fs::metadata(&safe_path).map_err(|e| {
                        ClawzError::Tool(format!("cannot stat '{}': {e}", safe_path.display()))
                    })?
                } else {
                    let bytes = self.backend.read_file(&safe_path).await?;
                    let content = String::from_utf8_lossy(&bytes);
                    return Ok(ToolResult {
                        tool_call_id: String::new(),
                        output: serde_json::json!({
                            "path": safe_path.to_string_lossy(),
                            "size_bytes": bytes.len(),
                            "content": content,
                            "backend": self.backend.backend_id(),
                        })
                        .to_string(),
                        is_error: false,
                    });
                };

                if metadata.len() as usize > MAX_READ_BYTES {
                    return Err(ClawzError::Tool(format!(
                        "file '{}' is too large ({} bytes, max {})",
                        safe_path.display(),
                        metadata.len(),
                        MAX_READ_BYTES
                    )));
                }

                let bytes = self.backend.read_file(&safe_path).await?;

                let encoding = args["encoding"].as_str().unwrap_or("utf8");
                let content = if encoding == "base64" {
                    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
                    format!("base64:{}", B64.encode(&bytes))
                } else {
                    String::from_utf8_lossy(&bytes).into_owned()
                };

                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "size_bytes": bytes.len(),
                    "content": content
                })
                .to_string()
            }

            "write_file" => {
                let content = args["content"].as_str().ok_or_else(|| {
                    ClawzError::Validation("content required for write_file".into())
                })?;

                if content.len() > MAX_WRITE_BYTES {
                    return Err(ClawzError::Tool(format!(
                        "content too large ({} bytes, max {})",
                        content.len(),
                        MAX_WRITE_BYTES
                    )));
                }

                // Ensure parent exists
                if let Some(parent) = safe_path.parent() {
                    if !parent.exists() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            ClawzError::Tool(format!("cannot create parent dir: {e}"))
                        })?;
                    }
                }

                let encoding = args["encoding"].as_str().unwrap_or("utf8");
                let bytes = if encoding == "base64" {
                    let stripped = content.strip_prefix("base64:").unwrap_or(content);
                    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
                    B64.decode(stripped)
                        .map_err(|e| ClawzError::Tool(format!("base64 decode failed: {e}")))?
                } else {
                    content.as_bytes().to_vec()
                };

                self.backend.write_file(&safe_path, &bytes).await?;

                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "bytes_written": bytes.len(),
                    "backend": self.backend.backend_id(),
                })
                .to_string()
            }

            "list_directory" if !self.uses_local_fs() => {
                let listing = self.backend.list_directory(&safe_path).await?;
                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "listing": listing,
                    "backend": self.backend.backend_id(),
                })
                .to_string()
            }

            "list_directory" => {
                let entries = std::fs::read_dir(&safe_path).map_err(|e| {
                    ClawzError::Tool(format!("cannot read dir '{}': {e}", safe_path.display()))
                })?;

                let items: Vec<Value> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let meta = e.metadata();
                        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                        serde_json::json!({
                            "name": e.file_name().to_string_lossy(),
                            "type": if is_dir { "directory" } else { "file" },
                            "size_bytes": size
                        })
                    })
                    .collect();

                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "entries": items,
                    "count": items.len()
                })
                .to_string()
            }

            "create_directory" if !self.uses_local_fs() => {
                let recursive = args["recursive"].as_bool().unwrap_or(true);
                self.backend.create_directory(&safe_path, recursive).await?;
                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "created": true,
                    "backend": self.backend.backend_id(),
                })
                .to_string()
            }

            "create_directory" => {
                let recursive = args["recursive"].as_bool().unwrap_or(true);
                if recursive {
                    std::fs::create_dir_all(&safe_path)
                        .map_err(|e| ClawzError::Tool(format!("mkdir -p failed: {e}")))?;
                } else {
                    std::fs::create_dir(&safe_path)
                        .map_err(|e| ClawzError::Tool(format!("mkdir failed: {e}")))?;
                }
                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "created": true
                })
                .to_string()
            }

            "delete_file" if !self.uses_local_fs() => {
                self.backend.delete_path(&safe_path).await?;
                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "deleted": true,
                    "backend": self.backend.backend_id(),
                })
                .to_string()
            }

            "delete_file" => {
                let meta = std::fs::metadata(&safe_path)
                    .map_err(|e| ClawzError::Tool(format!("cannot stat: {e}")))?;

                if meta.is_dir() {
                    std::fs::remove_dir_all(&safe_path)
                        .map_err(|e| ClawzError::Tool(format!("rmdir failed: {e}")))?;
                } else {
                    std::fs::remove_file(&safe_path)
                        .map_err(|e| ClawzError::Tool(format!("rm failed: {e}")))?;
                }
                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "deleted": true
                })
                .to_string()
            }

            "file_info" if !self.uses_local_fs() => {
                let out = self
                    .backend
                    .exec(
                        &format!(
                            "stat -- {}",
                            crate::terminal::shell_escape(safe_path.to_string_lossy().as_ref())
                        ),
                        None,
                        30,
                        &std::collections::HashMap::new(),
                    )
                    .await?;
                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "stat": out.stdout,
                    "backend": self.backend.backend_id(),
                })
                .to_string()
            }

            "file_info" => {
                let meta = std::fs::metadata(&safe_path)
                    .map_err(|e| ClawzError::Tool(format!("stat failed: {e}")))?;

                serde_json::json!({
                    "path": safe_path.to_string_lossy(),
                    "is_file": meta.is_file(),
                    "is_dir": meta.is_dir(),
                    "size_bytes": meta.len(),
                    "readonly": meta.permissions().readonly()
                })
                .to_string()
            }

            other => {
                return Err(ClawzError::Validation(format!(
                    "unknown operation: {other}"
                )));
            }
        };

        Ok(ToolResult {
            tool_call_id: String::new(),
            output,
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolConfig;
    use tempfile::TempDir;

    fn make_tool_with_tmp(dir: &TempDir) -> FileOpsTool {
        FileOpsTool {
            allowed_dirs: vec![dir.path().to_path_buf()],
            backend: Arc::new(LocalBackend::new(dir.path().to_path_buf())),
        }
    }

    fn make_ctx() -> ToolContext {
        ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: ToolConfig::default(),
        }
    }

    #[tokio::test]
    async fn test_write_and_read_file() {
        let tmp = TempDir::new().unwrap();
        let tool = make_tool_with_tmp(&tmp);
        let ctx = make_ctx();
        let file_path = tmp.path().join("test.txt");

        // Write
        let write_result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "write_file",
                    "path": file_path.to_str().unwrap(),
                    "content": "hello world"
                }),
            )
            .await
            .unwrap();
        assert!(!write_result.is_error);

        // Read
        let read_result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "read_file",
                    "path": file_path.to_str().unwrap()
                }),
            )
            .await
            .unwrap();
        assert!(!read_result.is_error);
        let parsed: Value = serde_json::from_str(&read_result.output).unwrap();
        assert_eq!(parsed["content"], "hello world");
    }

    #[tokio::test]
    async fn test_list_directory() {
        let tmp = TempDir::new().unwrap();
        let tool = make_tool_with_tmp(&tmp);
        let ctx = make_ctx();

        // Create a file
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "y").unwrap();

        let result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "list_directory",
                    "path": tmp.path().to_str().unwrap()
                }),
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(parsed["count"].as_u64().unwrap(), 2);
    }

    #[tokio::test]
    async fn test_sandbox_escape_blocked() {
        let tmp = TempDir::new().unwrap();
        let tool = make_tool_with_tmp(&tmp);
        let ctx = make_ctx();

        let result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "read_file",
                    "path": "/etc/passwd"
                }),
            )
            .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("sandbox") || msg.contains("outside"),
            "msg: {msg}"
        );
    }

    #[tokio::test]
    async fn test_create_and_delete_directory() {
        let tmp = TempDir::new().unwrap();
        let tool = make_tool_with_tmp(&tmp);
        let ctx = make_ctx();
        let new_dir = tmp.path().join("subdir");

        let result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "create_directory",
                    "path": new_dir.to_str().unwrap()
                }),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(new_dir.exists());

        // Delete
        let del_result = tool
            .execute(
                &ctx,
                serde_json::json!({
                    "operation": "delete_file",
                    "path": new_dir.to_str().unwrap()
                }),
            )
            .await
            .unwrap();
        assert!(!del_result.is_error);
        assert!(!new_dir.exists());
    }

    #[test]
    fn test_file_ops_name() {
        assert_eq!(FileOpsTool::new().name(), "file_ops");
    }
}
