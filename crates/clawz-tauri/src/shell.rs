//! Desktop shell connection mode — embedded worker vs remote gateway.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// How the embedded web UI talks to ClawZ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ShellMode {
    /// In-process worker IPC (`agent_chat`); dashboard REST expects a local gateway unless configured.
    #[default]
    Standalone,
    /// Remote gateway REST/WebSocket (`gateway_url` + API key).
    Gateway,
}

impl ShellMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "gateway" => Self::Gateway,
            _ => Self::Standalone,
        }
    }
}

/// Persisted + env-overridden shell settings exposed to the webview.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    pub mode: ShellMode,
    /// Gateway origin, e.g. `http://127.0.0.1:3000` (API base adds `/api/v1`).
    #[serde(default)]
    pub gateway_url: Option<String>,
    /// Whether an API key is stored in the OS keyring (value not returned).
    #[serde(default)]
    pub api_key_set: bool,
    /// Last known connection status for tray tooltip.
    #[serde(default)]
    pub connection_status: String,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            mode: ShellMode::default(),
            gateway_url: Some("http://127.0.0.1:3000".to_string()),
            api_key_set: false,
            connection_status: "idle".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfigInput {
    pub mode: String,
    #[serde(default)]
    pub gateway_url: Option<String>,
    /// When set, persisted to OS keyring; pass empty string to clear.
    #[serde(default)]
    pub api_key: Option<String>,
}

static RUNTIME_CONFIG: Mutex<Option<ShellConfig>> = Mutex::new(None);

pub fn load_config() -> ShellConfig {
    if let Ok(guard) = RUNTIME_CONFIG.lock() {
        if let Some(cfg) = guard.as_ref() {
            return cfg.clone();
        }
    }

    let mode = std::env::var("CLAWZ_SHELL_MODE")
        .ok()
        .map(|s| ShellMode::parse(&s))
        .unwrap_or_default();

    let gateway_url = std::env::var("CLAWZ_GATEWAY_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| Some("http://127.0.0.1:3000".to_string()));

    let api_key_set = read_keyring("api_key").is_ok();

    ShellConfig {
        mode,
        gateway_url,
        api_key_set,
        connection_status: "idle".to_string(),
    }
}

pub fn save_config(input: ShellConfigInput) -> Result<ShellConfig, String> {
    let mode = ShellMode::parse(&input.mode);

    if let Some(key) = input.api_key {
        if key.is_empty() {
            let _ = delete_keyring("api_key");
        } else {
            write_keyring("api_key", &key)?;
        }
    }

    let api_key_set = read_keyring("api_key").is_ok();

    let cfg = ShellConfig {
        mode,
        gateway_url: input
            .gateway_url
            .filter(|s| !s.is_empty())
            .or_else(|| Some("http://127.0.0.1:3000".to_string())),
        api_key_set,
        connection_status: "idle".to_string(),
    };

    if let Ok(mut guard) = RUNTIME_CONFIG.lock() {
        *guard = Some(cfg.clone());
    }

    Ok(cfg)
}

pub fn set_connection_status(status: impl Into<String>) {
    let status = status.into();
    if let Ok(mut guard) = RUNTIME_CONFIG.lock() {
        if let Some(cfg) = guard.as_mut() {
            cfg.connection_status = status;
        } else {
            let mut cfg = load_config();
            cfg.connection_status = status;
            *guard = Some(cfg);
        }
    }
}

pub fn read_keyring(account: &str) -> Result<String, String> {
    let entry = keyring::Entry::new("clawz-desktop", account).map_err(|e| e.to_string())?;
    entry.get_password().map_err(|e| e.to_string())
}

pub fn write_keyring(account: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new("clawz-desktop", account).map_err(|e| e.to_string())?;
    entry.set_password(value).map_err(|e| e.to_string())
}

pub fn delete_keyring(account: &str) -> Result<(), String> {
    let entry = keyring::Entry::new("clawz-desktop", account).map_err(|e| e.to_string())?;
    entry.delete_credential().map_err(|e| e.to_string())
}
