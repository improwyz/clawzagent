//! Operator config at `~/.clawz/config.json` (setup completion flag).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, SetupError};
use crate::paths::user_config_path;

/// User-level ClawZ config (distinct from gateway TOML / `cli.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClawzUserConfig {
    #[serde(default)]
    pub setup_complete: bool,
}

impl ClawzUserConfig {
    pub fn path() -> std::path::PathBuf {
        user_config_path()
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(path)?;
        serde_json::from_str(&data).map_err(|e| SetupError::serialization(e.to_string()))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| SetupError::serialization(e.to_string()))?;
        fs::write(path, data)?;
        Ok(())
    }

    pub fn mark_setup_complete(&mut self) -> Result<()> {
        self.setup_complete = true;
        self.save()
    }
}
