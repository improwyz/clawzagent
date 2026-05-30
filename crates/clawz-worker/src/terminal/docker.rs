//! Docker terminal backend — commands run inside a long-lived sandbox container.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, StartContainerOptions,
};
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use bollard::Docker;
use futures_util::StreamExt;

use clawz_core::error::{ClawzError, Result};

use super::{shell_escape, truncate_output, ExecResult, TerminalBackend, MAX_EXEC_OUTPUT_BYTES};

const CONTAINER_NAME: &str = "clawz-terminal-sandbox";
const CONTAINER_WORKDIR: &str = "/workspace";

pub struct DockerBackend {
    workdir: PathBuf,
    docker: Docker,
    image: String,
}

impl DockerBackend {
    pub fn new(workdir: PathBuf) -> Result<Self> {
        let image = std::env::var("CLAWZ_TERMINAL_DOCKER_IMAGE")
            .unwrap_or_else(|_| "alpine:3.19".into());
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ClawzError::Tool(format!("docker connect: {e}")))?;
        Ok(Self {
            workdir,
            docker,
            image,
        })
    }

    async fn ensure_container(&self) -> Result<String> {
        let filters = HashMap::from([("name", vec![CONTAINER_NAME])]);
        let listed = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters,
                ..Default::default()
            }))
            .await
            .map_err(|e| ClawzError::Tool(format!("docker list: {e}")))?;

        if let Some(c) = listed.first() {
            if let Some(id) = &c.id {
                if c.state.as_ref().is_some_and(|s| s != "running") {
                    self.docker
                        .start_container(id, None::<StartContainerOptions<String>>)
                        .await
                        .map_err(|e| ClawzError::Tool(format!("docker start: {e}")))?;
                }
                return Ok(id.clone());
            }
        }

        let host_workdir = self.workdir.to_string_lossy().to_string();
        std::fs::create_dir_all(&self.workdir).map_err(|e| {
            ClawzError::Tool(format!("create workdir {}: {e}", self.workdir.display()))
        })?;

        let mount = Mount {
            target: Some(CONTAINER_WORKDIR.into()),
            source: Some(host_workdir),
            typ: Some(MountTypeEnum::BIND),
            ..Default::default()
        };

        let config = Config {
            image: Some(self.image.clone()),
            hostname: Some(CONTAINER_NAME.into()),
            working_dir: Some(CONTAINER_WORKDIR.into()),
            host_config: Some(HostConfig {
                mounts: Some(vec![mount]),
                ..Default::default()
            }),
            cmd: Some(vec!["sleep".into(), "infinity".into()]),
            ..Default::default()
        };

        let created = self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: CONTAINER_NAME,
                    platform: None,
                }),
                config,
            )
            .await
            .map_err(|e| ClawzError::Tool(format!("docker create: {e}")))?;

        let id = created.id;
        self.docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ClawzError::Tool(format!("docker start: {e}")))?;

        Ok(id)
    }

    fn container_path(&self, path: &Path) -> String {
        if path.starts_with(&self.workdir) {
            let rel = path
                .strip_prefix(&self.workdir)
                .unwrap_or(path)
                .to_string_lossy();
            format!(
                "{}/{}",
                CONTAINER_WORKDIR,
                rel.trim_start_matches('/')
            )
        } else {
            format!("{}/{}", CONTAINER_WORKDIR, path.to_string_lossy())
        }
    }

    async fn exec_in_container(
        &self,
        command: &str,
        timeout_secs: u64,
    ) -> Result<ExecResult> {
        use bollard::exec::{CreateExecOptions, StartExecOptions};

        let container_id = self.ensure_container().await?;
        let exec = self
            .docker
            .create_exec(
                &container_id,
                CreateExecOptions {
                    cmd: Some(vec![
                        "sh".into(),
                        "-c".into(),
                        format!("cd {CONTAINER_WORKDIR} && {command}"),
                    ]),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| ClawzError::Tool(format!("docker create_exec: {e}")))?;

        let start = self
            .docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: false,
                    output_capacity: None,
                }),
            )
            .await
            .map_err(|e| ClawzError::Tool(format!("docker start_exec: {e}")))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let collect = async {
            if let bollard::exec::StartExecResults::Attached { mut output, .. } = start {
                while let Some(chunk) = output.next().await {
                    match chunk.map_err(|e| ClawzError::Tool(format!("docker exec stream: {e}")))? {
                        bollard::container::LogOutput::StdOut { message } => {
                            stdout.extend_from_slice(&message);
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            stderr.extend_from_slice(&message);
                        }
                        _ => {}
                    }
                }
            }
            Ok::<(), ClawzError>(())
        };

        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), collect)
            .await
            .map_err(|_| ClawzError::Tool(format!("docker exec timed out after {timeout_secs}s")))?
            .map_err(|e| e)?;

        let inspect = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| ClawzError::Tool(format!("docker inspect_exec: {e}")))?;

        let exit_code = inspect
            .exit_code
            .map(|c| i32::try_from(c).unwrap_or(-1))
            .unwrap_or(-1);

        Ok(ExecResult {
            stdout: truncate_output(&stdout, MAX_EXEC_OUTPUT_BYTES),
            stderr: truncate_output(&stderr, MAX_EXEC_OUTPUT_BYTES),
            exit_code,
        })
    }
}

#[async_trait]
impl TerminalBackend for DockerBackend {
    fn backend_id(&self) -> &str {
        "docker"
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
        self.exec_in_container(command, timeout_secs).await
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let cp = shell_escape(&self.container_path(path));
        let out = self
            .exec_in_container(&format!("cat -- {cp}"), 60)
            .await?;
        if out.success() {
            Ok(out.stdout.into_bytes())
        } else {
            Err(ClawzError::Tool(out.stderr))
        }
    }

    async fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let cp = shell_escape(&self.container_path(path));
        let encoded = B64.encode(content);
        let cmd = format!(
            "mkdir -p -- $(dirname {cp}) && echo {data} | base64 -d > {cp}",
            data = shell_escape(&encoded),
            cp = cp
        );
        let out = self.exec_in_container(&cmd, 120).await?;
        if out.success() {
            Ok(())
        } else {
            Err(ClawzError::Tool(out.stderr))
        }
    }
}
