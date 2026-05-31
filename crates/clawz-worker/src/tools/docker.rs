//! Docker container management tool.
//!
//! This module implements [`DockerToolManager`], which provides agents with
//! container lifecycle management, log streaming, and filesystem access via
//! the Bollard Docker client.
//!
//! ## Features
//!
//! - **Container creation**: Launch containers with environment, volumes, and port mappings
//! - **Lifecycle**: Start, stop, restart, remove containers
//! - **Monitoring**: Stream logs in real-time, inspect container state
//! - **Environment injection**: Pass environment variables and secrets securely
//! - **Health checks**: Query container health status
//! - **Tool library**: Pre-defined images for common utilities (node, python, etc.)
//!
//! ## Cross-module dependencies
//!
//! // Dependency: Requires Docker daemon (local or remote socket).
//! // Dependency: Uses `bollard` crate for Docker client.
//! // Dependency: Implements the [`Tool`] trait from [`super::tool_trait`].

use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogOutput, LogsOptions,
    RemoveContainerOptions, RestartContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::{ContainerStateStatusEnum, HostConfig, PortBinding, RestartPolicy};
use clawz_core::error::ClawzError;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for deploying a tool container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContainerConfig {
    /// Image to run (e.g., "postgres:16-alpine")
    pub image: String,
    /// Port mappings: container_port -> host_port (0 = auto-assign)
    pub port_mappings: Vec<PortMapping>,
    /// Environment variables
    pub env_vars: Vec<(String, String)>,
    /// Volume mounts: host_path -> container_path
    pub volumes: Vec<VolumeMount>,
    /// Memory limit in bytes (0 = unlimited)
    pub memory_limit_bytes: i64,
    /// CPU quota in nano-CPUs (0 = unlimited, 1_000_000_000 = 1 CPU)
    pub cpu_nano_cpus: i64,
    /// Container name (auto-generated if empty)
    pub name: Option<String>,
    /// Restart policy
    pub restart_policy: ContainerRestartPolicy,
    /// Custom labels
    pub labels: HashMap<String, String>,
    /// Command override
    pub command: Option<Vec<String>>,
}

impl Default for ToolContainerConfig {
    fn default() -> Self {
        Self {
            image: String::new(),
            port_mappings: Vec::new(),
            env_vars: Vec::new(),
            volumes: Vec::new(),
            memory_limit_bytes: 0,
            cpu_nano_cpus: 0,
            name: None,
            restart_policy: ContainerRestartPolicy::OnFailure,
            labels: HashMap::new(),
            command: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub container_port: u16,
    pub host_port: u16,
    pub protocol: String, // "tcp" or "udp"
}

impl PortMapping {
    pub fn tcp(container_port: u16, host_port: u16) -> Self {
        Self {
            container_port,
            host_port,
            protocol: "tcp".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum ContainerRestartPolicy {
    No,
    Always,
    #[default]
    OnFailure,
    UnlessStopped,
}

impl ContainerRestartPolicy {
    #[allow(dead_code)]
    fn to_bollard_policy(&self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Always => "always",
            Self::OnFailure => "on-failure",
            Self::UnlessStopped => "unless-stopped",
        }
    }
}

/// Represents a running tool container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningTool {
    pub container_id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub ports: Vec<MappedPort>,
    pub health: ToolHealth,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedPort {
    pub container_port: u16,
    pub host_port: u16,
    pub protocol: String,
}

/// Health status of a running tool container.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolHealth {
    Healthy,
    Unhealthy,
    Starting,
    Unknown,
}

impl std::fmt::Display for ToolHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Starting => write!(f, "starting"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// An entry in the Docker tool catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerToolEntry {
    pub name: String,
    pub image: String,
    pub ports: Vec<u16>,
    pub category: String,
    pub description: String,
    pub default_env: Vec<(String, String)>,
}

impl DockerToolEntry {
    fn simple(name: &str, image: &str, ports: Vec<u16>, category: &str, description: &str) -> Self {
        Self {
            name: name.into(),
            image: image.into(),
            ports,
            category: category.into(),
            description: description.into(),
            default_env: Vec::new(),
        }
    }

    fn with_env(mut self, key: &str, value: &str) -> Self {
        self.default_env.push((key.into(), value.into()));
        self
    }
}

/// HealthStatus alias for backwards compatibility.
pub type HealthStatus = ToolHealth;

/// A catalog of known Docker tools.
pub struct ToolCatalog;

impl ToolCatalog {
    pub fn all() -> Vec<DockerToolEntry> {
        docker_tool_library()
    }

    pub fn find(name: &str) -> Option<DockerToolEntry> {
        docker_tool_library().into_iter().find(|e| e.name == name)
    }

    pub fn by_category(category: &str) -> Vec<DockerToolEntry> {
        docker_tool_library()
            .into_iter()
            .filter(|e| e.category == category)
            .collect()
    }
}

/// Returns the full catalog of pre-configured Docker tools.
pub fn docker_tool_library() -> Vec<DockerToolEntry> {
    vec![
        // Databases
        DockerToolEntry::simple(
            "postgres",
            "postgres:16-alpine",
            vec![5432],
            "database",
            "PostgreSQL relational database",
        )
        .with_env("POSTGRES_PASSWORD", "clawz")
        .with_env("POSTGRES_USER", "clawz")
        .with_env("POSTGRES_DB", "clawz"),
        DockerToolEntry::simple(
            "redis",
            "redis:7-alpine",
            vec![6379],
            "database",
            "Redis in-memory data store",
        ),
        DockerToolEntry::simple(
            "mongodb",
            "mongo:7",
            vec![27017],
            "database",
            "MongoDB document database",
        )
        .with_env("MONGO_INITDB_ROOT_USERNAME", "clawz")
        .with_env("MONGO_INITDB_ROOT_PASSWORD", "clawz"),
        DockerToolEntry::simple(
            "elasticsearch",
            "elasticsearch:8.13.0",
            vec![9200, 9300],
            "search",
            "Elasticsearch full-text search engine",
        )
        .with_env("discovery.type", "single-node")
        .with_env("xpack.security.enabled", "false"),
        // Message Queues
        DockerToolEntry::simple(
            "rabbitmq",
            "rabbitmq:3-management-alpine",
            vec![5672, 15672],
            "queue",
            "RabbitMQ message broker with management UI",
        ),
        DockerToolEntry::simple(
            "nats",
            "nats:2-alpine",
            vec![4222, 8222],
            "queue",
            "NATS lightweight messaging system",
        ),
        // Monitoring
        DockerToolEntry::simple(
            "prometheus",
            "prom/prometheus:latest",
            vec![9090],
            "monitoring",
            "Prometheus metrics collection",
        ),
        DockerToolEntry::simple(
            "grafana",
            "grafana/grafana:latest",
            vec![3000],
            "monitoring",
            "Grafana observability dashboards",
        )
        .with_env("GF_SECURITY_ADMIN_PASSWORD", "clawz"),
        DockerToolEntry::simple(
            "jaeger",
            "jaegertracing/all-in-one:latest",
            vec![16686, 14268, 6831, 6832],
            "monitoring",
            "Jaeger distributed tracing",
        ),
        // Web / Proxy
        DockerToolEntry::simple(
            "nginx",
            "nginx:alpine",
            vec![80, 443],
            "web",
            "NGINX web server and reverse proxy",
        ),
        // Storage
        DockerToolEntry::simple(
            "minio",
            "quay.io/minio/minio:latest",
            vec![9000, 9001],
            "storage",
            "MinIO S3-compatible object storage",
        )
        .with_env("MINIO_ROOT_USER", "minioadmin")
        .with_env("MINIO_ROOT_PASSWORD", "minioadmin"),
        // Security
        DockerToolEntry::simple(
            "vault",
            "hashicorp/vault:latest",
            vec![8200],
            "security",
            "HashiCorp Vault secrets management",
        )
        .with_env("VAULT_DEV_ROOT_TOKEN_ID", "clawz-dev-token"),
        // Communication / Testing
        DockerToolEntry::simple(
            "mailhog",
            "mailhog/mailhog:latest",
            vec![1025, 8025],
            "communication",
            "MailHog email testing tool",
        ),
        // Code Quality
        DockerToolEntry::simple(
            "sonarqube",
            "sonarqube:community",
            vec![9000],
            "code_analysis",
            "SonarQube code quality analysis",
        ),
        // AI / ML
        DockerToolEntry::simple(
            "ollama",
            "ollama/ollama:latest",
            vec![11434],
            "ai",
            "Ollama local LLM inference server",
        ),
        DockerToolEntry::simple(
            "chromadb",
            "chromadb/chroma:latest",
            vec![8000],
            "ai",
            "ChromaDB vector database",
        ),
        // Additional useful tools
        DockerToolEntry::simple(
            "postgres-timescale",
            "timescale/timescaledb-ha:pg16-latest",
            vec![5432],
            "database",
            "TimescaleDB + pgvector on PostgreSQL 16",
        )
        .with_env("POSTGRES_PASSWORD", "clawz")
        .with_env("POSTGRES_USER", "clawz")
        .with_env("POSTGRES_DB", "clawz"),
        DockerToolEntry::simple(
            "qdrant",
            "qdrant/qdrant:latest",
            vec![6333, 6334],
            "ai",
            "Qdrant vector similarity search",
        ),
        DockerToolEntry::simple(
            "kafka",
            "confluentinc/cp-kafka:7.7.0",
            vec![9092],
            "queue",
            "Apache Kafka distributed event streaming",
        )
        .with_env("KAFKA_BROKER_ID", "1")
        .with_env("KAFKA_ZOOKEEPER_CONNECT", "zookeeper:2181")
        .with_env("KAFKA_ADVERTISED_LISTENERS", "PLAINTEXT://localhost:9092"),
        DockerToolEntry::simple(
            "traefik",
            "traefik:v3",
            vec![80, 443, 8080],
            "web",
            "Traefik cloud-native API gateway",
        ),
    ]
}

/// Docker Tool Manager — manages containerized tools via the Docker API.
#[allow(dead_code)]
pub struct DockerToolManager {
    docker: Docker,
    /// Label applied to all containers managed by this instance
    managed_label: String,
}

impl DockerToolManager {
    /// Connect to the local Docker daemon.
    pub fn new() -> Result<Self, ClawzError> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ClawzError::Tool(format!("failed to connect to Docker: {e}")))?;

        Ok(Self {
            docker,
            managed_label: "managed-by=clawz".into(),
        })
    }

    /// Connect to a remote Docker daemon.
    pub fn with_uri(uri: &str) -> Result<Self, ClawzError> {
        let docker = Docker::connect_with_http_defaults()
            .map_err(|e| ClawzError::Tool(format!("failed to connect to Docker at {uri}: {e}")))?;

        Ok(Self {
            docker,
            managed_label: "managed-by=clawz".into(),
        })
    }

    /// Pull an image if not already present locally.
    pub async fn pull_image(&self, image: &str) -> Result<(), ClawzError> {
        log::info!("Pulling Docker image: {image}");

        let mut stream = self.docker.create_image(
            Some(CreateImageOptions {
                from_image: image,
                ..Default::default()
            }),
            None,
            None,
        );

        while let Some(output) = stream.next().await {
            match output {
                Ok(info) => {
                    if let Some(status) = &info.status {
                        log::debug!("Docker pull {image}: {status}");
                    }
                }
                Err(e) => {
                    return Err(ClawzError::Tool(format!(
                        "failed to pull image '{image}': {e}"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Deploy a containerized tool.
    /// Auto-pulls the image if not present. Returns the container ID.
    pub async fn deploy_tool(&self, config: ToolContainerConfig) -> Result<String, ClawzError> {
        let image = &config.image;

        // Check if image exists locally; pull if not
        match self.docker.inspect_image(image).await {
            Ok(_) => {} // already present
            Err(_) => {
                self.pull_image(image).await?;
            }
        }

        // Build port bindings
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();

        for pm in &config.port_mappings {
            let key = format!("{}/{}", pm.container_port, pm.protocol);
            let binding = PortBinding {
                host_ip: Some("0.0.0.0".into()),
                host_port: Some(if pm.host_port == 0 {
                    "".into() // auto-assign
                } else {
                    pm.host_port.to_string()
                }),
            };
            port_bindings.insert(key.clone(), Some(vec![binding]));
            exposed_ports.insert(key, HashMap::new());
        }

        // Build env vars
        let env: Vec<String> = config
            .env_vars
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        // Build volume bindings
        let binds: Vec<String> = config
            .volumes
            .iter()
            .map(|v| {
                if v.read_only {
                    format!("{}:{}:ro", v.host_path, v.container_path)
                } else {
                    format!("{}:{}", v.host_path, v.container_path)
                }
            })
            .collect();

        // Build labels
        let mut labels = config.labels.clone();
        labels.insert("managed-by".into(), "clawz".into());
        labels.insert("clawz-tool".into(), "true".into());

        // Restart policy
        let restart_policy = RestartPolicy {
            name: Some(match &config.restart_policy {
                ContainerRestartPolicy::No => bollard::models::RestartPolicyNameEnum::NO,
                ContainerRestartPolicy::Always => bollard::models::RestartPolicyNameEnum::ALWAYS,
                ContainerRestartPolicy::OnFailure => {
                    bollard::models::RestartPolicyNameEnum::ON_FAILURE
                }
                ContainerRestartPolicy::UnlessStopped => {
                    bollard::models::RestartPolicyNameEnum::UNLESS_STOPPED
                }
            }),
            maximum_retry_count: Some(5),
        };

        let host_config = HostConfig {
            port_bindings: Some(port_bindings),
            binds: if binds.is_empty() { None } else { Some(binds) },
            memory: if config.memory_limit_bytes > 0 {
                Some(config.memory_limit_bytes)
            } else {
                None
            },
            nano_cpus: if config.cpu_nano_cpus > 0 {
                Some(config.cpu_nano_cpus)
            } else {
                None
            },
            restart_policy: Some(restart_policy),
            ..Default::default()
        };

        let container_config = Config {
            image: Some(image.as_str()),
            env: if env.is_empty() {
                None
            } else {
                Some(env.iter().map(|s| s.as_str()).collect())
            },
            exposed_ports: if exposed_ports.is_empty() {
                None
            } else {
                Some(
                    exposed_ports
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.clone()))
                        .collect(),
                )
            },
            host_config: Some(host_config),
            labels: Some(
                labels
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect(),
            ),
            cmd: config
                .command
                .as_ref()
                .map(|c| c.iter().map(|s| s.as_str()).collect()),
            ..Default::default()
        };

        let container_name = config.name.as_deref().map(|n| n.to_string());

        let create_opts = container_name
            .as_deref()
            .map(|name| CreateContainerOptions {
                name,
                platform: None,
            });

        let response = self
            .docker
            .create_container(create_opts, container_config)
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to create container: {e}")))?;

        let container_id = response.id;

        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to start container: {e}")))?;

        log::info!("Deployed container {container_id} (image: {image})");
        Ok(container_id)
    }

    /// Stop a running container.
    pub async fn stop_tool(&self, container_id: &str) -> Result<(), ClawzError> {
        self.docker
            .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to stop container: {e}")))?;

        Ok(())
    }

    /// Restart a container.
    pub async fn restart_tool(&self, container_id: &str) -> Result<(), ClawzError> {
        self.docker
            .restart_container(container_id, Some(RestartContainerOptions { t: 10 }))
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to restart container: {e}")))?;

        Ok(())
    }

    /// Remove a container (stop first if running).
    pub async fn remove_tool(&self, container_id: &str) -> Result<(), ClawzError> {
        // Stop first (ignore error if already stopped)
        self.stop_tool(container_id).await.ok();

        self.docker
            .remove_container(
                container_id,
                Some(RemoveContainerOptions {
                    v: true, // remove volumes
                    force: true,
                    link: false,
                }),
            )
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to remove container: {e}")))?;

        Ok(())
    }

    /// List all running tool containers managed by ClawZ.
    pub async fn list_running(&self) -> Result<Vec<RunningTool>, ClawzError> {
        let mut filters = HashMap::new();
        filters.insert("label", vec!["managed-by=clawz"]);

        let containers = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: false, // only running
                filters,
                ..Default::default()
            }))
            .await
            .map_err(|e| ClawzError::Tool(format!("failed to list containers: {e}")))?;

        let mut tools = Vec::new();
        for c in containers {
            let container_id = c.id.unwrap_or_default();
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|s| s.trim_start_matches('/').to_string())
                .unwrap_or_else(|| container_id[..12].to_string());

            let image = c.image.unwrap_or_default();
            let status = c.status.unwrap_or_default();

            let ports = c
                .ports
                .unwrap_or_default()
                .iter()
                .map(|p| MappedPort {
                    container_port: p.private_port,
                    host_port: p.public_port.unwrap_or(0),
                    protocol: p
                        .typ
                        .as_ref()
                        .map(|t| format!("{t:?}").to_lowercase())
                        .unwrap_or_else(|| "tcp".into()),
                })
                .collect();

            let health = self.health_check(&container_id).await;
            let labels = c.labels.unwrap_or_default();

            tools.push(RunningTool {
                container_id,
                name,
                image,
                status,
                ports,
                health,
                labels,
            });
        }

        Ok(tools)
    }

    /// Get health status of a container.
    pub async fn health_check(&self, container_id: &str) -> ToolHealth {
        match self.docker.inspect_container(container_id, None).await {
            Ok(info) => {
                match info.state {
                    Some(state) => {
                        match state.status {
                            Some(ContainerStateStatusEnum::RUNNING) => {
                                // Check health if available
                                if let Some(health) = state.health {
                                    match health.status {
                                        Some(bollard::models::HealthStatusEnum::HEALTHY) => {
                                            ToolHealth::Healthy
                                        }
                                        Some(bollard::models::HealthStatusEnum::UNHEALTHY) => {
                                            ToolHealth::Unhealthy
                                        }
                                        Some(bollard::models::HealthStatusEnum::STARTING) => {
                                            ToolHealth::Starting
                                        }
                                        _ => ToolHealth::Healthy, // running without healthcheck = healthy
                                    }
                                } else {
                                    ToolHealth::Healthy
                                }
                            }
                            Some(ContainerStateStatusEnum::EXITED)
                            | Some(ContainerStateStatusEnum::DEAD) => ToolHealth::Unhealthy,
                            _ => ToolHealth::Starting,
                        }
                    }
                    None => ToolHealth::Unknown,
                }
            }
            Err(_) => ToolHealth::Unknown,
        }
    }

    /// Retrieve recent logs from a container.
    pub async fn tool_logs(
        &self,
        container_id: &str,
        lines: usize,
    ) -> Result<Vec<String>, ClawzError> {
        let options = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            tail: lines.to_string(),
            ..Default::default()
        };

        let mut stream = self.docker.logs(container_id, Some(options));
        let mut log_lines = Vec::new();

        while let Some(output) = stream.next().await {
            match output {
                Ok(LogOutput::StdOut { message }) => {
                    log_lines.push(format!(
                        "STDOUT: {}",
                        String::from_utf8_lossy(&message).trim()
                    ));
                }
                Ok(LogOutput::StdErr { message }) => {
                    log_lines.push(format!(
                        "STDERR: {}",
                        String::from_utf8_lossy(&message).trim()
                    ));
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(ClawzError::Tool(format!("log stream error: {e}")));
                }
            }
        }

        Ok(log_lines)
    }

    /// Monitor all managed containers and restart unhealthy ones.
    pub async fn monitor_and_heal(&self) -> Result<Vec<String>, ClawzError> {
        let running = self.list_running().await?;
        let mut restarted = Vec::new();

        for tool in &running {
            if tool.health == ToolHealth::Unhealthy {
                log::warn!(
                    "Container {} ({}) is unhealthy — restarting",
                    tool.name,
                    &tool.container_id[..12]
                );
                match self.restart_tool(&tool.container_id).await {
                    Ok(_) => {
                        log::info!("Restarted container {}", tool.name);
                        restarted.push(tool.container_id.clone());
                    }
                    Err(e) => {
                        log::error!("Failed to restart container {}: {e}", tool.name);
                    }
                }
            }
        }

        Ok(restarted)
    }

    /// Deploy a tool from the catalog by name.
    pub async fn deploy_from_catalog(
        &self,
        tool_name: &str,
        name_override: Option<String>,
    ) -> Result<String, ClawzError> {
        let entry = ToolCatalog::find(tool_name).ok_or_else(|| ClawzError::NotFound {
            entity: "Docker tool".into(),
            id: tool_name.into(),
        })?;

        let port_mappings = entry
            .ports
            .iter()
            .map(|&p| PortMapping::tcp(p, 0)) // 0 = auto-assign host port
            .collect();

        let config = ToolContainerConfig {
            image: entry.image.clone(),
            port_mappings,
            env_vars: entry.default_env,
            name: name_override.or_else(|| Some(format!("clawz-{tool_name}"))),
            ..Default::default()
        };

        self.deploy_tool(config).await
    }
}

impl Default for DockerToolManager {
    fn default() -> Self {
        Self::new().expect("failed to connect to Docker daemon")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_tool_library_not_empty() {
        let lib = docker_tool_library();
        assert!(!lib.is_empty());
        // Verify required entries exist
        assert!(lib.iter().any(|e| e.name == "postgres"));
        assert!(lib.iter().any(|e| e.name == "redis"));
        assert!(lib.iter().any(|e| e.name == "ollama"));
        assert!(lib.iter().any(|e| e.name == "grafana"));
    }

    #[test]
    fn test_catalog_find() {
        let entry = ToolCatalog::find("postgres");
        assert!(entry.is_some());
        let e = entry.unwrap();
        assert_eq!(e.name, "postgres");
        assert!(e.ports.contains(&5432));
    }

    #[test]
    fn test_catalog_by_category() {
        let dbs = ToolCatalog::by_category("database");
        assert!(!dbs.is_empty());
        assert!(dbs.iter().all(|e| e.category == "database"));
    }

    #[test]
    fn test_port_mapping_tcp() {
        let pm = PortMapping::tcp(5432, 5432);
        assert_eq!(pm.container_port, 5432);
        assert_eq!(pm.host_port, 5432);
        assert_eq!(pm.protocol, "tcp");
    }

    #[test]
    fn test_restart_policy_names() {
        assert_eq!(ContainerRestartPolicy::Always.to_bollard_policy(), "always");
        assert_eq!(
            ContainerRestartPolicy::OnFailure.to_bollard_policy(),
            "on-failure"
        );
        assert_eq!(ContainerRestartPolicy::No.to_bollard_policy(), "no");
    }

    #[test]
    fn test_tool_health_display() {
        assert_eq!(format!("{}", ToolHealth::Healthy), "healthy");
        assert_eq!(format!("{}", ToolHealth::Unhealthy), "unhealthy");
    }

    #[test]
    fn test_tool_container_config_default() {
        let cfg = ToolContainerConfig::default();
        assert_eq!(cfg.memory_limit_bytes, 0);
        assert_eq!(cfg.cpu_nano_cpus, 0);
        assert!(cfg.env_vars.is_empty());
    }
}
