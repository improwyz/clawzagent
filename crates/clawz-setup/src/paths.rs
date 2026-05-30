//! Canonical paths under `~/.clawz` (or `CLAWZ_HOME`).

use std::path::PathBuf;

/// Root operator data directory (`CLAWZ_HOME` or `~/.clawz`).
pub fn clawz_home() -> PathBuf {
    std::env::var("CLAWZ_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".clawz")
        })
}

pub fn setup_dir() -> PathBuf {
    clawz_home().join("setup")
}

pub fn session_path() -> PathBuf {
    setup_dir().join("session.json")
}

pub fn user_config_path() -> PathBuf {
    clawz_home().join("config.json")
}
