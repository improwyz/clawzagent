//! Deploy planning: prebuilt vs build, image tag, standalone source path.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::spec::HostSpecReport;
use crate::types::{DeploymentChoice, InstallStrategy};

/// Compose overlay selection (mirrors `install-common.sh` `compose_args`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposeOverlay {
    Base,
    Prebuilt,
    Build,
}

/// Planned install actions for the setup wizard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployPlan {
    pub deployment: DeploymentChoice,
    pub strategy: InstallStrategy,
    pub compose_overlay: Option<ComposeOverlay>,
    /// Env vars to export / write (e.g. `CLAWZ_IMAGE_TAG`).
    pub env: BTreeMap<String, String>,
    /// `cargo run` / source path when `standalone` + `source`.
    pub standalone_source: Option<StandaloneSourcePlan>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Source-run instructions for `CLAWZ_MODE=standalone` without Docker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandaloneSourcePlan {
    pub repo_root: PathBuf,
    pub gateway_cmd: String,
    pub worker_cmd: String,
    pub env: BTreeMap<String, String>,
}

/// Errors that block deploy until the operator fixes prerequisites.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeployPlanError {
    #[error("plan has blocking errors: {0}")]
    Blocked(String),
}

/// Maps deployment mode + install strategy to compose overlays and env.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeployPlanner;

impl DeployPlanner {
    pub fn plan(
        deployment: DeploymentChoice,
        strategy: InstallStrategy,
        spec: &HostSpecReport,
        image_tag: Option<&str>,
    ) -> Result<DeployPlan, DeployPlanError> {
        let tag = image_tag.unwrap_or("latest").to_string();
        let mut env = BTreeMap::new();
        env.insert("CLAWZ_IMAGE_TAG".into(), tag.clone());
        env.insert(
            "CLAWZ_REGISTRY".into(),
            std::env::var("CLAWZ_REGISTRY").unwrap_or_else(|_| "ghcr.io/improwyz".into()),
        );
        env.insert(
            "CLAWZ_AGENT_IMAGE".into(),
            format!(
                "{}/clawz-agent:{}",
                env.get("CLAWZ_REGISTRY")
                    .map(String::as_str)
                    .unwrap_or("ghcr.io/improwyz"),
                tag
            ),
        );

        let mut warnings = spec.warnings.clone();
        warnings.extend(spec.threshold_warnings(deployment));

        let mut errors = Vec::new();
        let (compose_overlay, standalone_source) = match (deployment, strategy) {
            (DeploymentChoice::Standalone, InstallStrategy::Source) => {
                env.insert("CLAWZ_MODE".into(), "standalone".into());
                let repo = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let source = StandaloneSourcePlan {
                    repo_root: repo.clone(),
                    gateway_cmd: "cargo run -p clawz-gateway".into(),
                    worker_cmd: "cargo run -p clawz-worker".into(),
                    env: BTreeMap::from([("CLAWZ_MODE".into(), "standalone".into())]),
                };
                if spec.rust_version.is_none() {
                    errors.push("Rust toolchain required for standalone source install.".into());
                } else if spec.rust_meets_minimum == Some(false) {
                    errors.push("Rust version below minimum for source builds.".into());
                }
                (None, Some(source))
            }
            (DeploymentChoice::Standalone, InstallStrategy::Prebuilt | InstallStrategy::Build) => {
                warnings.push(
                    "Standalone mode with Docker images is unusual; prefer source or micro.".into(),
                );
                let overlay = overlay_for_strategy(strategy);
                validate_docker_strategy(strategy, spec, &mut errors);
                (Some(overlay), None)
            }
            (DeploymentChoice::Micro | DeploymentChoice::Elastic, strategy) => {
                env.insert(
                    "CLAWZ_MODE".into(),
                    match deployment {
                        DeploymentChoice::Micro => "micro",
                        DeploymentChoice::Elastic => "elastic",
                        DeploymentChoice::Standalone => "standalone",
                    }
                    .into(),
                );
                let overlay = overlay_for_strategy(strategy);
                validate_docker_strategy(strategy, spec, &mut errors);
                if matches!(strategy, InstallStrategy::Prebuilt) && !spec.github_token_present {
                    errors.push(
                        "GITHUB_TOKEN is required to pull private GHCR prebuilt images.".into(),
                    );
                }
                (Some(overlay), None)
            }
        };

        let plan = DeployPlan {
            deployment,
            strategy,
            compose_overlay,
            env,
            standalone_source,
            warnings,
            errors,
        };

        if !plan.errors.is_empty() {
            return Err(DeployPlanError::Blocked(plan.errors.join("; ")));
        }
        Ok(plan)
    }
}

fn overlay_for_strategy(strategy: InstallStrategy) -> ComposeOverlay {
    match strategy {
        InstallStrategy::Prebuilt => ComposeOverlay::Prebuilt,
        InstallStrategy::Build => ComposeOverlay::Build,
        InstallStrategy::Source => ComposeOverlay::Base,
    }
}

fn validate_docker_strategy(
    strategy: InstallStrategy,
    spec: &HostSpecReport,
    errors: &mut Vec<String>,
) {
    if matches!(strategy, InstallStrategy::Source) {
        return;
    }
    if !spec.docker_available {
        errors.push("Docker is required for prebuilt and build install strategies.".into());
    }
    if spec.docker_available && !spec.compose_v2_available {
        errors.push("Docker Compose v2 is required.".into());
    }
}

impl DeployPlan {
    /// Compose CLI `-f` arguments (e.g. `-f docker-compose.yml -f docker-compose.prebuilt.yml`).
    pub fn compose_file_args(&self) -> Vec<&'static str> {
        match self.compose_overlay {
            None => vec![],
            Some(ComposeOverlay::Base) => vec!["-f", "docker-compose.yml"],
            Some(ComposeOverlay::Prebuilt) => {
                vec![
                    "-f",
                    "docker-compose.yml",
                    "-f",
                    "docker-compose.prebuilt.yml",
                ]
            }
            Some(ComposeOverlay::Build) => {
                vec!["-f", "docker-compose.yml", "-f", "docker-compose.build.yml"]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::HostSpecReport;

    fn sample_spec(docker: bool, gh_token: bool, rust_ok: bool) -> HostSpecReport {
        HostSpecReport {
            os: "Linux".into(),
            arch: "x86_64".into(),
            ram_mb: Some(16 * 1024),
            disk_free_gb: Some(50),
            cpu_count: Some(8),
            docker_available: docker,
            docker_detail: None,
            compose_v2_available: docker,
            compose_detail: None,
            rust_version: if rust_ok { Some("1.87.0".into()) } else { None },
            rust_meets_minimum: if rust_ok { Some(true) } else { None },
            node_version: None,
            github_token_present: gh_token,
            summary: String::new(),
            warnings: vec![],
        }
    }

    #[test]
    fn prebuilt_micro_sets_tag_and_overlay() {
        let spec = sample_spec(true, true, true);
        let plan = DeployPlanner::plan(
            DeploymentChoice::Micro,
            InstallStrategy::Prebuilt,
            &spec,
            Some("v1.0.0"),
        )
        .unwrap();
        assert_eq!(
            plan.env.get("CLAWZ_IMAGE_TAG").map(String::as_str),
            Some("v1.0.0")
        );
        assert_eq!(plan.compose_overlay, Some(ComposeOverlay::Prebuilt));
        assert!(plan.standalone_source.is_none());
    }

    #[test]
    fn prebuilt_without_gh_token_fails() {
        let spec = sample_spec(true, false, true);
        let err = DeployPlanner::plan(
            DeploymentChoice::Micro,
            InstallStrategy::Prebuilt,
            &spec,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn standalone_source_sets_mode_and_commands() {
        let spec = sample_spec(false, false, true);
        let plan = DeployPlanner::plan(
            DeploymentChoice::Standalone,
            InstallStrategy::Source,
            &spec,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.env.get("CLAWZ_MODE").map(String::as_str),
            Some("standalone")
        );
        assert!(plan.compose_overlay.is_none());
        let source = plan.standalone_source.expect("source plan");
        assert!(source.gateway_cmd.contains("clawz-gateway"));
    }

    #[test]
    fn build_strategy_uses_build_overlay() {
        let spec = sample_spec(true, true, true);
        let plan =
            DeployPlanner::plan(DeploymentChoice::Micro, InstallStrategy::Build, &spec, None)
                .unwrap();
        assert_eq!(plan.compose_overlay, Some(ComposeOverlay::Build));
        let args = plan.compose_file_args();
        assert!(args.contains(&"docker-compose.build.yml"));
    }
}
