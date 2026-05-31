//! Operator config at `~/.clawz/cli.toml` (gateway URL, API key).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(default = "default_gateway_url")]
    pub gateway_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_worker_url")]
    pub worker_url: String,
}

fn default_gateway_url() -> String {
    "http://127.0.0.1:3000".to_string()
}

fn default_worker_url() -> String {
    "http://127.0.0.1:50051".to_string()
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            gateway_url: default_gateway_url(),
            api_key: None,
            worker_url: default_worker_url(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var("CLAWZ_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".clawz")
        })
}

pub fn config_path() -> PathBuf {
    config_dir().join("cli.toml")
}

pub fn load() -> Result<CliConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(CliConfig::default());
    }
    let data =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&data).context("parse cli.toml")
}

pub fn save(cfg: &CliConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = toml::to_string_pretty(cfg).context("serialize cli.toml")?;
    std::fs::write(&path, data).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Merge file config with environment overrides.
pub fn resolve() -> CliConfig {
    let mut cfg = load().unwrap_or_default();
    if let Ok(url) = std::env::var("CLAWZ_GATEWAY_URL") {
        if !url.is_empty() {
            cfg.gateway_url = url;
        }
    }
    if let Ok(key) = std::env::var("CLAWZ_API_KEY") {
        if !key.is_empty() {
            cfg.api_key = Some(key);
        }
    }
    if cfg.api_key.is_none() {
        if let Ok(keys) = std::env::var("VALID_API_KEYS") {
            if let Some(first) = keys.split(',').next() {
                let k = first.trim();
                if !k.is_empty() {
                    cfg.api_key = Some(k.to_string());
                }
            }
        }
    }
    if let Ok(url) = std::env::var("WORKER_URL") {
        if !url.is_empty() {
            cfg.worker_url = url;
        }
    }
    cfg
}

pub fn api_v1_base(cfg: &CliConfig) -> String {
    let u = cfg.gateway_url.trim_end_matches('/');
    if u.ends_with("/api/v1") {
        u.to_string()
    } else {
        format!("{u}/api/v1")
    }
}
