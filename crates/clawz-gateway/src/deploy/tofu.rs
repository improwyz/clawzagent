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
