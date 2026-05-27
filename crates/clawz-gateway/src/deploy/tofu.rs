use crate::deploy::common::generate_deployment_id;
use crate::deploy::provider::{
    DeployConfig, DeployMode, DeployProvider, DeploymentInfo, DeploymentStatus, ProviderCredentials,
};
use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use std::path::PathBuf;
use std::process::Command;
use tokio::fs;

pub struct TofuRunner {
    work_dir: PathBuf,
}

impl TofuRunner {
    pub fn new(work_dir: impl Into<PathBuf>) -> Self {
        Self {
            work_dir: work_dir.into(),
        }
    }

    pub async fn init(&self) -> Result<()> {
        let output = Command::new("tofu")
            .args(["init", "-input=false"])
            .current_dir(&self.work_dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ClawzError::Internal(
                        "OpenTofu (tofu) not found in PATH. Install from https://opentofu.org".into(),
                    )
                } else {
                    ClawzError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ClawzError::Internal(format!("tofu init failed: {}", stderr)));
        }

        Ok(())
    }

    pub async fn plan(&self) -> Result<String> {
        let output = Command::new("tofu")
            .args(["plan", "-input=false", "-out=tfplan"])
            .current_dir(&self.work_dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ClawzError::Internal("OpenTofu (tofu) not found in PATH".into())
                } else {
                    ClawzError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ClawzError::Internal(format!("tofu plan failed: {}", stderr)));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub async fn apply(&self, auto_approve: bool) -> Result<serde_json::Value> {
        let mut args = vec!["apply", "-input=false", "-json"];
        if auto_approve {
            args.push("-auto-approve");
        } else {
            args.push("tfplan");
        }

        let output = Command::new("tofu")
            .args(&args)
            .current_dir(&self.work_dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ClawzError::Internal("OpenTofu (tofu) not found in PATH".into())
                } else {
                    ClawzError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ClawzError::Internal(format!(
                "tofu apply failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut results = Vec::new();

        for line in stdout.lines() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                results.push(value);
            }
        }

        Ok(serde_json::Value::Array(results))
    }

    pub async fn destroy(&self, auto_approve: bool) -> Result<serde_json::Value> {
        let mut args = vec!["destroy", "-input=false", "-json"];
        if auto_approve {
            args.push("-auto-approve");
        }

        let output = Command::new("tofu")
            .args(&args)
            .current_dir(&self.work_dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ClawzError::Internal("OpenTofu (tofu) not found in PATH".into())
                } else {
                    ClawzError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ClawzError::Internal(format!(
                "tofu destroy failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut results = Vec::new();

        for line in stdout.lines() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                results.push(value);
            }
        }

        Ok(serde_json::Value::Array(results))
    }

    pub async fn output(&self, name: Option<&str>) -> Result<serde_json::Value> {
        let mut args = vec!["output", "-json"];
        if let Some(n) = name {
            args.push(n);
        }

        let output = Command::new("tofu")
            .args(&args)
            .current_dir(&self.work_dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ClawzError::Internal("OpenTofu (tofu) not found in PATH".into())
                } else {
                    ClawzError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ClawzError::Internal(format!(
                "tofu output failed: {}",
                stderr
            )));
        }

        serde_json::from_slice(&output.stdout)
            .map_err(|e| ClawzError::Serialization(e.to_string()))
    }

    pub async fn write_main_tf(&self, content: &str) -> Result<()> {
        let path = self.work_dir.join("main.tf");
        fs::write(&path, content)
            .await
            .map_err(ClawzError::Io)?;
        Ok(())
    }

    pub async fn write_variables_tf(&self, content: &str) -> Result<()> {
        let path = self.work_dir.join("variables.tf");
        fs::write(&path, content)
            .await
            .map_err(ClawzError::Io)?;
        Ok(())
    }

    pub async fn write_tfvars(&self, content: &str) -> Result<()> {
        let path = self.work_dir.join("terraform.tfvars");
        fs::write(&path, content)
            .await
            .map_err(ClawzError::Io)?;
        Ok(())
    }
}

pub fn generate_docker_service_tf(image: &str, port: u16, replicas: u32) -> String {
    format!(
        r#"terraform {{
  required_providers {{
    docker = {{
      source  = "kreuzwerker/docker"
      version = "~> 3.0"
    }}
  }}
}}

provider "docker" {{}}

resource "docker_image" "app" {{
  name         = "{}"
  keep_locally = true
}}

resource "docker_container" "app" {{
  count = {}
  image = docker_image.app.image_id
  name  = "clawz-app-${{count.index}}"

  ports {{
    internal = {}
    external = {} + count.index
  }}
}}
"#,
        image,
        replicas,
        port,
        port
    )
}

/// OpenTofu-backed deploy provider (local `tofu` CLI).
pub struct TofuDeployAdapter {
    base_work_dir: PathBuf,
}

impl TofuDeployAdapter {
    pub fn new(base_work_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_work_dir: base_work_dir.into(),
        }
    }
}

#[async_trait]
impl DeployProvider for TofuDeployAdapter {
    fn provider_id(&self) -> &str {
        "opentofu"
    }

    fn display_name(&self) -> &str {
        "OpenTofu"
    }

    fn supported_modes(&self) -> Vec<DeployMode> {
        vec![
            DeployMode::Docker { image: String::new() },
            DeployMode::NativeBinary,
        ]
    }

    async fn validate_credentials(&self, _creds: &ProviderCredentials) -> Result<()> {
        let output = Command::new("tofu").arg("version").output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ClawzError::Internal(
                    "OpenTofu (tofu) not found in PATH. Install from https://opentofu.org".into(),
                )
            } else {
                ClawzError::Io(e)
            }
        })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(ClawzError::Internal("tofu version check failed".into()))
        }
    }

    async fn deploy(&self, config: &DeployConfig) -> Result<DeploymentInfo> {
        let id = generate_deployment_id("tofu");
        let work_dir = self.base_work_dir.join(id.replace('/', "_"));
        fs::create_dir_all(&work_dir).await.map_err(ClawzError::Io)?;

        let runner = TofuRunner::new(&work_dir);
        let main_tf = if let Some(custom) = config.env_vars.get("__MAIN_TF") {
            custom.clone()
        } else {
            let image = match &config.mode {
                DeployMode::Docker { image } => image.clone(),
                DeployMode::NativeBinary => "debian:bullseye-slim".into(),
                DeployMode::Wasm => {
                    return Err(ClawzError::Validation(
                        "OpenTofu adapter does not support Wasm; use Fastly or Cloudflare".into(),
                    ))
                }
            };
            generate_docker_service_tf(&image, 8080, config.replicas.max(1))
        };

        runner.write_main_tf(&main_tf).await?;
        runner.init().await?;
        runner.apply(true).await?;

        Ok(DeploymentInfo {
            id,
            url: format!("file://{}", work_dir.display()),
            status: DeploymentStatus::Pending,
            ..Default::default()
        })
    }

    async fn status(&self, _id: &str) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus::Running)
    }

    async fn destroy(&self, id: &str, _external_resource: Option<&str>) -> Result<()> {
        let work_dir = self.base_work_dir.join(id.replace('/', "_"));
        if work_dir.exists() {
            let runner = TofuRunner::new(&work_dir);
            let _ = runner.destroy(true).await;
            let _ = fs::remove_dir_all(&work_dir).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_docker_service_tf() {
        let tf = generate_docker_service_tf("myapp:latest", 8080, 2);
        assert!(tf.contains("docker_image"));
        assert!(tf.contains("myapp:latest"));
        assert!(tf.contains("count = 2"));
        assert!(tf.contains("8080"));
    }
}
